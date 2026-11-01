"""OBJ 软渲染（评估用，不参与训练）：z-buffer 光栅化 + 按 vt 采样贴图 + 朗伯明暗。

用法：
  py -3.11 scripts/render_obj.py <mesh.obj> <out.png> [--size 384] [--yaw 35] [--pitch 25]
                                [--tex out.png] [--bg 1.0]

输出：打印轮廓像素数（用于两版网格的削薄量对比）。
"""
from __future__ import annotations

import argparse
import sys

import cv2
import numpy as np


def load_obj(path):
    verts, uvs, faces, fuv = [], [], [], []
    for line in open(path, encoding="utf-8"):
        p = line.split()
        if not p:
            continue
        if p[0] == "v":
            verts.append([float(x) for x in p[1:4]])
        elif p[0] == "vt":
            uvs.append([float(x) for x in p[1:3]])
        elif p[0] == "f":
            faces.append([int(t.split("/")[0]) - 1 for t in p[1:4]])
            fuv.append([int(t.split("/")[1]) - 1 for t in p[1:4]])
    return (
        np.array(verts, np.float64),
        np.array(uvs, np.float64),
        np.array(faces, np.int32),
        np.array(fuv, np.int32),
    )


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("obj")
    ap.add_argument("out")
    ap.add_argument("--size", type=int, default=384)
    ap.add_argument("--yaw", type=float, default=35.0)
    ap.add_argument("--pitch", type=float, default=25.0)
    ap.add_argument("--tex", default=None, help="贴图 PNG（默认取 obj 同名的 .png）")
    ap.add_argument("--bg", type=float, default=1.0)
    a = ap.parse_args()

    V, VT, F, FT = load_obj(a.obj)
    tex_path = a.tex or a.obj.rsplit(".", 1)[0] + ".png"
    try:
        tex = cv2.cvtColor(cv2.imread(tex_path), cv2.COLOR_BGR2RGB).astype(np.float32) / 255.0
    except Exception:
        tex = None
    th, tw = (tex.shape[:2] if tex is not None else (1, 1))

    yaw, pitch = np.deg2rad(a.yaw), np.deg2rad(a.pitch)
    Ry = np.array([
        [np.cos(yaw), 0, np.sin(yaw)],
        [0, 1, 0],
        [-np.sin(yaw), 0, np.cos(yaw)],
    ])
    Rx = np.array([
        [1, 0, 0],
        [0, np.cos(pitch), -np.sin(pitch)],
        [0, np.sin(pitch), np.cos(pitch)],
    ])
    c = (V.min(0) + V.max(0)) / 2
    P = (V - c) @ Ry.T @ Rx.T
    P = P / np.abs(P).max() * 2.0
    H = W = a.size
    sx = (P[:, 0] * 0.85 + 0.5) * W
    sy = (0.5 - P[:, 1] * 0.85) * H
    # 光照方向：与视线一致的左上前方
    L = np.array([-0.4, 0.6, 0.7])
    L = L / np.linalg.norm(L)

    img = np.full((H, W, 3), a.bg, np.float32)
    zbuf = np.full((H, W), -1e9)
    hit = np.zeros((H, W), bool)
    for t, tt in zip(F, FT):
        x, y, z = sx[t], sy[t], P[t, 2]
        x0, x1 = int(np.floor(x.min())), int(np.ceil(x.max()))
        y0, y1 = int(np.floor(y.min())), int(np.ceil(y.max()))
        x0, y0 = max(x0, 0), max(y0, 0)
        x1, y1 = min(x1, W - 1), min(y1, H - 1)
        if x1 < x0 or y1 < y0:
            continue
        den = (y[1] - y[2]) * (x[0] - x[2]) + (x[2] - x[1]) * (y[0] - y[2])
        if abs(den) < 1e-12:
            continue
        xs, ys = np.meshgrid(np.arange(x0, x1 + 1) + 0.5, np.arange(y0, y1 + 1) + 0.5)
        w0 = ((y[1] - y[2]) * (xs - x[2]) + (x[2] - x[1]) * (ys - y[2])) / den
        w1 = ((y[2] - y[0]) * (xs - x[2]) + (x[0] - x[2]) * (ys - y[2])) / den
        w2 = 1.0 - w0 - w1
        m = (w0 >= -1e-6) & (w1 >= -1e-6) & (w2 >= -1e-6)
        if not m.any():
            continue
        zz = w0 * z[0] + w1 * z[1] + w2 * z[2]
        if tex is not None:
            u = w0 * VT[tt[0], 0] + w1 * VT[tt[1], 0] + w2 * VT[tt[2], 0]
            v = w0 * VT[tt[0], 1] + w1 * VT[tt[1], 1] + w2 * VT[tt[2], 1]
            tx = np.mod(u * tw, tw).astype(int).clip(0, tw - 1)
            ty = np.mod((1.0 - v) * th, th).astype(int).clip(0, th - 1)
            col = tex[ty, tx] * 0.85 + 0.15
        else:
            col = np.full(xs.shape + (3,), 0.8, np.float32)
        sub_z, sub_i, sub_h = zbuf[y0:y1 + 1, x0:x1 + 1], img[y0:y1 + 1, x0:x1 + 1], hit[y0:y1 + 1, x0:x1 + 1]
        upd = m & (zz > sub_z)
        sub_z[upd] = zz[upd]
        sub_i[upd] = col[upd]
        sub_h[upd] = True
    cv2.imwrite(a.out, cv2.cvtColor((np.clip(img, 0, 1) * 255).astype(np.uint8), cv2.COLOR_RGB2BGR))
    print(f"{a.obj} -> {a.out}  轮廓像素 {int(hit.sum())}  张角 yaw={a.yaw} pitch={a.pitch}")


if __name__ == "__main__":
    sys.exit(main())

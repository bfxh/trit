"""S1 场景合成器：程序化部件库 + 装配布局 → objects.json + 清单 parts.json。

只造数据，不碰训练。每场景：
- 部件数 24–80，种类 cube / cylinder / sphere / torus，逐部件随机缩放与旋转；
- 布局从 5 种里选：平板 / 塔 / 环 / 团簇 / 链；
- 输出 `<scene>/objects.json`（渲染器输入）与 `<scene>/parts.json`（id→base-3 digits 清单）。
"""
from __future__ import annotations

import argparse
import json
import math
import random
from pathlib import Path

KINDS = ["part_cube", "cylinder", "sphere", "torus"]
# 材质类（语义标签，和形状解耦；渲染器按它上 PBR 参数，清单里一并落盘）
MATERIALS = ["wood", "metal", "stone", "fabric"]


def gen_cluster(rng: random.Random, n: int, sep: float = 0.95):
    """紧凑团块：第一个部件在原点，之后每个部件贴在随机一个已放部件的外侧
    （距离 = (r_i + r_j) × sep；sep<1 表示略微咬合），返回 [(center, kind, scale, rot)]。"""
    kinds = [rng.choice(KINDS) for _ in range(n)]
    scales = []
    radii = []
    for kind in kinds:
        if kind == "part_cube":
            s0 = rng.uniform(3.5, 7.0)
            sc = (s0, s0, s0)
            base = 0.055
        elif kind == "cylinder":
            s0 = rng.uniform(0.4, 0.9)
            sc = (s0, s0, rng.uniform(0.5, 1.4))
            base = 0.9
        elif kind == "sphere":
            s0 = rng.uniform(0.3, 0.7)
            sc = (s0, s0, s0)
            base = 0.7
        else:
            s0 = rng.uniform(0.4, 0.8)
            sc = (s0, s0, s0)
            base = 0.68
        scales.append(sc)
        radii.append(base * max(sc) * 0.5)
    centers = [(0.0, 0.0, 0.0)]
    for i in range(1, n):
        j = rng.randrange(i)
        d = (rng.uniform(-1, 1), rng.uniform(-1, 1), rng.uniform(-1, 1))
        norm = math.sqrt(sum(v * v for v in d)) or 1.0
        d = tuple(v / norm for v in d)
        dist = (radii[i] + radii[j]) * sep
        c = tuple(centers[j][ax] + d[ax] * dist for ax in range(3))
        centers.append(c)
    out = []
    for i in range(n):
        rot = (rng.uniform(0, 180), rng.uniform(0, 180), rng.uniform(0, 360))
        out.append((centers[i], kinds[i], scales[i], rot))
    return out


def gen_layout_multi(rng: random.Random, objects: int, per_obj: tuple[int, int],
                     sep: float = 1.25, gap: float = 0.2,
                     ext_range: tuple[float, float] = (0.55, 1.0)):
    """生成 `objects` 个物体（各自一团部件），返回 (parts, 每物体的部件数)。
    每簇先按近贴合间距（sep×半径和）摆放，再整簇等比缩放到 ext_range 目标尺寸，
    最后落地并沿 x 排开，物体之间留 0.2 m 空隙。"""
    parts = []
    sizes = []
    centers_used = []
    for k in range(objects):
        n = rng.randint(per_obj[0], per_obj[1])
        cl = gen_cluster(rng, n, sep=sep)
        if not cl:
            continue
        # 尺寸/间距一律按**表面**算：部件中心 ± 自身半径
        rads = [part_radius(kd, sc) for _c, kd, sc, _ro in cl]
        lo = [min(c[a] - r for (c, *_rest), r in zip(cl, rads)) for a in range(3)]
        hi = [max(c[a] + r for (c, *_rest), r in zip(cl, rads)) for a in range(3)]
        ext = max(hi[a] - lo[a] for a in range(3))
        ext = max(ext, 0.25)
        s = rng.uniform(*ext_range) / ext
        cl = [((c[0] * s, c[1] * s, c[2] * s), kd,
               (sc[0] * s, sc[1] * s, sc[2] * s), ro) for c, kd, sc, ro in cl]
        rads = [r * s for r in rads]
        lo = [min(c[a] - r for (c, *_rest), r in zip(cl, rads)) for a in range(3)]
        hi = [max(c[a] + r for (c, *_rest), r in zip(cl, rads)) for a in range(3)]
        r = ext * s / 2
        cx = 0.0 if k == 0 else centers_used[-1][0] + centers_used[-1][1] + r + gap
        shift = (cx - (lo[0] + hi[0]) / 2, 0.2 - (lo[1] + hi[1]) / 2, 0.05 - lo[2])
        cl = [((c[0] + shift[0], c[1] + shift[1], c[2] + shift[2]), kd, sc, ro) for c, kd, sc, ro in cl]
        parts.extend(cl)
        sizes.append(len(cl))
        centers_used.append((cx, r))
    return parts, sizes


def part_radius(kind: str, scale) -> float:
    """部件保守外接球半径（与渲染原语一致）。"""
    base = {"part_cube": 0.055, "cylinder": 0.9, "sphere": 0.7, "torus": 0.68}[kind]
    return base * max(scale) * 0.5


def gen_layout(rng: random.Random, n: int, kind_layout: str, sep: float = 1.6):
    """先定每个部件的尺寸，再按最小间距采样位置（避免互相穿插），返回
    [(center, kind, scale, rot)]；放不下就提前停（少几个部件）。"""
    kinds = [rng.choice(KINDS) for _ in range(n)]
    scales = []
    radii = []
    for kind in kinds:
        if kind == "part_cube":
            # 渲染器的 part_cube 原语边长 0.055 m；这里按 3.5–7 倍缩放 ⇒ 0.19–0.39 m，
            # 与其他原语（球/柱/环 0.4–1 m）同量级，避免退化成 4 px 杂点
            s0 = rng.uniform(3.5, 7.0)
            sc = (s0, s0, s0)
            base = 0.055
        elif kind == "cylinder":
            s0 = rng.uniform(0.4, 0.9)
            sc = (s0, s0, rng.uniform(0.5, 1.4))
            base = 0.9
        elif kind == "sphere":
            s0 = rng.uniform(0.3, 0.7)
            sc = (s0, s0, s0)
            base = 0.7
        else:
            s0 = rng.uniform(0.4, 0.8)
            sc = (s0, s0, s0)
            base = 0.68
        scales.append(sc)
        radii.append(base * max(sc) * 0.5)
    placed = []
    span = {"slab": (1.1, 1.1, 0.25), "tower": (0.5, 0.5, 1.3), "ring": (1.4, 1.4, 0.3),
            "cluster": (1.3, 1.3, 1.0), "chain": (1.2, 1.2, 0.9)}[kind_layout]
    tries = 0
    i = 0
    while i < n and tries < n * 400:
        tries += 1
        c = (rng.uniform(-span[0], span[0]), rng.uniform(-span[1], span[1]), rng.uniform(0.1, span[2] + 0.6))
        ok = True
        for (c2, r2) in placed:
            d = math.dist(c, c2)
            if d < (radii[i] + r2) * sep:
                ok = False
                break
        if not ok:
            continue
        placed.append((c, radii[i]))
        i += 1
    out = []
    for idx, (c, _r) in enumerate(placed):
        rot = (rng.uniform(0, 180), rng.uniform(0, 180), rng.uniform(0, 360))
        out.append((c, kinds[idx], scales[idx], rot))
    return out


def digits_of(idx: int, k: int):
    out = []
    x = idx
    for _ in range(k):
        out.append(x % 3)
        x //= 3
    return list(reversed(out))


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True)
    ap.add_argument("--scenes", type=int, default=200)
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--min-parts", type=int, default=16)
    ap.add_argument("--max-parts", type=int, default=40)
    ap.add_argument("--objects-min", type=int, default=0,
                    help="每场景物体数下限；0 = 单物体（旧布局）")
    ap.add_argument("--objects-max", type=int, default=0)
    ap.add_argument("--obj-sep", type=float, default=1.25,
                    help="物体内部件最小间距 = (r1+r2)×sep；<1 表示部件互相咬合")
    ap.add_argument("--obj-gap", type=float, default=0.2, help="物体间空隙（米）")
    ap.add_argument("--obj-min-parts", type=int, default=3)
    ap.add_argument("--obj-max-parts", type=int, default=7)
    args = ap.parse_args()

    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    index = []
    for si in range(args.scenes):
        rng = random.Random(args.seed * 100003 + si)
        if args.objects_max > 0:
            n_obj = rng.randint(max(1, args.objects_min), args.objects_max)
            parts, sizes = gen_layout_multi(
                rng, n_obj, (args.obj_min_parts, args.obj_max_parts),
                sep=args.obj_sep, gap=args.obj_gap,
            )
            layout = f"multi{n_obj}"
        else:
            n = rng.randint(args.min_parts, args.max_parts)
            layout = rng.choice(["slab", "tower", "ring", "cluster", "chain"])
            parts = gen_layout(rng, n, layout)
            sizes = [len(parts)]
        n = len(parts)
        if args.objects_max == 0:
            xs = [p[0][0] for p in parts]
            ys = [p[0][1] for p in parts]
            zs = [p[0][2] for p in parts]
            ext = max(max(xs) - min(xs), max(ys) - min(ys), max(zs) - min(zs), 0.4)
            radius = max(1.8, ext * rng.uniform(2.2, 3.2))
        else:
            # 多物体：半径按**表面**包围盒，避免部件半径把画面撑出框
            rads = [part_radius(kd, sc) for _c, kd, sc, _ro in parts]
            lo = [min(c[a] - r for (c, *_rest), r in zip(parts, rads)) for a in range(3)]
            hi = [max(c[a] + r for (c, *_rest), r in zip(parts, rads)) for a in range(3)]
            ext = max(hi[a] - lo[a] for a in range(3))
            radius = max(2.0, ext * 1.6)
        elev = rng.uniform(10.0, 60.0)
        # 把场景包围盒中心对齐渲染器固定的注视点 (0.2, 0.2, 0.6)
        if parts:
            xs = [p[0][0] for p in parts]
            ys = [p[0][1] for p in parts]
            zs = [p[0][2] for p in parts]
            dx = 0.2 - (min(xs) + max(xs)) / 2
            dy = 0.2 - (min(ys) + max(ys)) / 2
            # z 不强行居中：只把最低点抬到离地 5cm，避免部件沉进地板被挡住
            dz = max(0.0, 0.05 - min(zs)) + (0.0 if min(zs) >= 0.05 else 0.0)
            parts = [((c[0] + dx, c[1] + dy, c[2] + dz), k, sc, ro) for (c, k, sc, ro) in parts]
        sd = out / f"scene{si:04d}"
        sd.mkdir(parents=True, exist_ok=True)

        objs = []
        for i, (c, kind, scale, rot) in enumerate(parts, start=1):
            objs.append(
                {
                    "name": f"p{i:03d}",
                    "pass_index": i,
                    "center": [round(v, 4) for v in c],
                    "kind": kind,
                    "probe": [round(v, 4) for v in c],
                    "scale": [round(v, 3) for v in scale],
                    "rot": [round(v, 1) for v in rot],
                    "material": MATERIALS[rng.randrange(len(MATERIALS))],
                }
            )
        (sd / "objects.json").write_text(json.dumps(objs, ensure_ascii=False), encoding="utf-8")

        k = 2
        while 3**k < n:
            k += 1
        man = {
            "k": k,
            "capacity": 3**k,
            "groups": [{"path": "asm", "parent": None}],
            "parts": [
                {
                    "id": i,
                    "name": f"p{i:03d}",
                    "path": f"asm/p{i:03d}",
                    "digits": digits_of(i - 1, k),
                    "parent": "asm",
                }
                for i in range(1, n + 1)
            ],
        }
        (sd / "parts.json").write_text(json.dumps(man, ensure_ascii=False), encoding="utf-8")
        inst = []
        first = 1
        for j, m in enumerate(sizes):
            inst.append({"instance": j, "first_id": first, "last_id": first + m - 1, "n_parts": m})
            first += m
        (sd / "instances.json").write_text(
            json.dumps({"instances": inst}, ensure_ascii=False), encoding="utf-8"
        )
        index.append(
            {
                "scene": f"scene{si:04d}",
                "n_parts": n,
                "objects": len(sizes),
                "layout": layout,
                "radius": round(radius, 3),
                "elev": round(elev, 1),
            }
        )
    (out / "scenes.json").write_text(json.dumps(index, ensure_ascii=False, indent=1), encoding="utf-8")
    print(f"scenes={len(index)} -> {out}")


if __name__ == "__main__":
    main()

"""按 mat60 的相机分布重渲 mat240（修域偏移）。

mat60 每场景的 radius/elev 是逐场景不同的（3.9–8.08 / 10–47°），
mat240 当初用渲染器默认（3.4 / 12°）⇒ 前景占比 0.35 vs 0.11，成了另一个域。

做法：从 mat60 的 60 个 manifest 里自举采样 (radius, elev)，
并保证 radius ≥ 1.35 × 场景簇半径（放得下），逐场景调 blender 重渲。

用法：python scripts/rerender_matched.py --root E:/trit-data/s1/mat240
"""
from __future__ import annotations

import argparse
import glob
import json
import math
import random
import statistics
import subprocess
from pathlib import Path

BLENDER = r"C:\Program Files\Blender Foundation\Blender 4.2\blender.exe"
GEN = Path(__file__).with_name("blender_gt.py")


def cluster_radius(objects: list[dict]) -> float:
    centers = [o["center"] for o in objects]
    cx = [statistics.mean(v[i] for v in centers) for i in range(3)]
    r = 0.0
    for o in objects:
        d = math.dist(o["center"], cx)
        r = max(r, d + max(o["scale"]) * 0.5)
    return r


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--root", required=True)
    ap.add_argument("--ref", default="E:/trit-data/s1/mat60/renders")
    ap.add_argument("--scenes", default=None, help="场景根目录（默认 <root>/scenes）")
    ap.add_argument("--frames", type=int, default=8)
    ap.add_argument("--res", type=int, default=384)
    args = ap.parse_args()

    root = Path(args.root)
    scenes = Path(args.scenes) if args.scenes else root / "scenes"
    ref = sorted(glob.glob(str(Path(args.ref) / "scene*" / "manifest.json")))
    cams = [json.load(open(p))["config"] for p in ref]
    pairs = [(c["radius"], c["elev_deg"]) for c in cams if c.get("radius")]
    rng = random.Random(7)

    dirs = sorted(d for d in (root / "renders").glob("pt*") if (scenes / d.name / "objects.json").exists())
    print(f"{len(dirs)} 场景；相机样本 {len(pairs)} 个", flush=True)
    for i, d in enumerate(dirs):
        objs = json.load(open(scenes / d.name / "objects.json"))
        r_need = cluster_radius(objs) * 1.35
        radius, elev = rng.choice(pairs)
        radius = max(radius, r_need)
        subprocess.run(
            [
                BLENDER, "-b", "--factory-startup", "-P", str(GEN), "--",
                "--out", str(d),
                "--objects-json", str(scenes / d.name / "objects.json"),
                "--frames", str(args.frames), "--res", str(args.res),
                "--samples", "16", "--device", "GPU",
                "--radius", f"{radius:.3f}", "--elev-deg", f"{elev:.2f}",
            ],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=True,
        )
        if i % 20 == 0:
            print(f"[{i}/{len(dirs)}] {d.name} R={radius:.2f} elev={elev:.1f}", flush=True)
    print("DONE", flush=True)


if __name__ == "__main__":
    main()

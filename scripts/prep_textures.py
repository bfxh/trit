"""把 ambientCG 的 Color 贴图转成 256² 灰度 PNG（N5 训练用）。

用法：python scripts/prep_textures.py [IN_DIR] [OUT_DIR] [SIZE]
  IN_DIR  默认 D:/KF/trit-data/textures/ambientcg（其下每个子目录含 *Color*.jpg）
  OUT_DIR 默认 D:/KF/trit-data/textures/png256
  SIZE    默认 256（方形缩放到 SIZE×SIZE）
数据准备脚本，不参与训练。
"""
from __future__ import annotations

import sys
from pathlib import Path

from PIL import Image


def main() -> int:
    src = Path(sys.argv[1] if len(sys.argv) > 1 else "D:/KF/trit-data/textures/ambientcg")
    dst = Path(sys.argv[2] if len(sys.argv) > 2 else "D:/KF/trit-data/textures/png256")
    size = int(sys.argv[3]) if len(sys.argv) > 3 else 256
    dst.mkdir(parents=True, exist_ok=True)
    n = 0
    for p in sorted(src.rglob("*")):
        if not p.is_file() or "color" not in p.name.lower():
            continue
        if p.suffix.lower() not in (".jpg", ".jpeg", ".png"):
            continue
        im = Image.open(p).convert("L").resize((size, size), Image.LANCZOS)
        out = dst / (p.parent.name + "_" + p.stem + ".png")
        im.save(out)
        n += 1
    print(f"PREP {n} 张 {size}² 灰度 → {dst}")
    return 0 if n > 0 else 1


if __name__ == "__main__":
    sys.exit(main())

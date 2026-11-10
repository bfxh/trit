"""用 Blender（外部加载器）导入导出的 OBJ，核对几何、材质与贴图是否真的可用。

用法：
  "C:/Program Files/Blender Foundation/Blender 4.2/blender.exe" -b -P scripts/check_obj_blender.py -- \
      --obj E:/tmp/batch/scene0010.obj

输出（stdout，供脚本抓取）：
  CHECK_OBJ verts=<n> tris=<n> loose_edges=<n> materials=<n> images=<n> image_size=<w>x<h> uv_layers=<n> uv_range=<min>..<max>
"""
from __future__ import annotations

import sys

import bpy


def main() -> int:
    argv = sys.argv[sys.argv.index("--") + 1 :] if "--" in sys.argv else []
    obj = None
    for i, a in enumerate(argv):
        if a == "--obj":
            obj = argv[i + 1]
    if not obj:
        print("usage: --obj <file.obj>", file=sys.stderr)
        return 2

    bpy.ops.wm.read_factory_settings(use_empty=True)
    bpy.ops.wm.obj_import(filepath=obj, validate_meshes=True)

    objects = [o for o in bpy.data.objects if o.type == "MESH"]
    if not objects:
        print("CHECK_OBJ FAIL 没有导入任何网格")
        return 1
    verts = sum(len(o.data.vertices) for o in objects)
    tris = sum(sum(1 for p in o.data.polygons if len(p.vertices) == 3) for o in objects)
    ngons = sum(sum(1 for p in o.data.polygons if len(p.vertices) > 3) for o in objects)
    uv_layers = sum(len(o.data.uv_layers) for o in objects)
    umin, umax = 1e9, -1e9
    for o in objects:
        if not o.data.uv_layers:
            continue
        for d in o.data.uv_layers[0].data:
            umin = min(umin, d.uv[0])
            umax = max(umax, d.uv[0])
    mats = {m.name for o in objects for m in o.data.materials if m}
    images = [i for i in bpy.data.images if i.source == "FILE" and i.size[0] > 0]
    img_desc = ",".join(f"{i.name}:{i.size[0]}x{i.size[1]}" for i in images) or "none"
    print(
        f"CHECK_OBJ verts={verts} tris={tris} ngons={ngons} materials={len(mats)} "
        f"uv_layers={uv_layers} uv_u={umin:.3f}..{umax:.3f} images={img_desc}"
    )
    ok = verts > 0 and tris > 0 and len(mats) >= 1 and images and uv_layers >= 1
    print("CHECK_OBJ PASS" if ok else "CHECK_OBJ FAIL")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())

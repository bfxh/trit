"""Blender headless 合成数据管线：RGB + 深度 + 法线 + 分割 + 相机位姿。

用法（Blender 4.2，自带 numpy）：
  blender -b --factory-startup -P blender_gt.py -- \
      --out D:/KF/.yanagent/drift-data/scene01 --frames 16 --res 512 --samples 16 --device GPU

设计约束（与报告 §6 第 1 步对齐）：
- 场景与相机轨迹**确定性**：无随机几何、固定 Cycles seed、固定帧步进；
- GT 全部来自渲染 pass，不是近似：Depth = **相机空间 Z**（正值，视线方向），
  Normal = **世界空间**法线（4.2 实测），Seg = Object Index（每个物体的 pass_index）；
  主点按 Blender 像素中心取 (res-1)/2；
- 位姿给到 float64：K（3×3）与 T_wc（4×4，Blender camera→world）。

输出（每帧）：
  rgb/000000.png      sRGB 8bit
  depth/000000.npy    float32 H×W，相机空间 Z（米）
  normal/000000.npy   float32 H×W×3，相机空间单位法线
  seg/000000.npy      int32 H×W，pass_index（0 = 无物体/背景）
  meta/000000.json    K / T_wc / T_cw / 相机参数 / 帧号
  manifest.json       全量配置 + 每帧 meta + 场景物体中心（验收用）
"""

from __future__ import annotations

import argparse
import colorsys
import random
import json
import math
import sys
import time
from pathlib import Path

import bpy
import numpy as np
from mathutils import Vector

# ---------------------------------------------------------------- 场景常量

# 布局 A/B/C：同一套物体类型，不同摆位、相机半径与仰角。
# 多场景的目的不是比难度，而是看「0.2 m 体素下界」是否随场景几何漂移。
LAYOUTS = {
    "A": {
        "radius": 4.2, "elev_deg": 20.0,
        "objects": [
            ("floor", 1, (0.0, 0.0, 0.0), "plane", (0.0, 0.0, 0.0)),
            ("cube", 2, (-1.5, -1.0, 0.5), "cube", (-1.5, -1.0, 0.5)),
            ("sphere", 3, (1.4, -0.8, 0.7), "sphere", (1.4, -0.8, 0.7)),
            ("cylinder", 4, (0.4, 1.5, 0.6), "cylinder", (0.4, 1.5, 0.6)),
            ("torus", 5, (2.0, 1.6, 0.45), "torus", (2.5, 1.6, 0.45)),
        ],
    },
    "B": {  # 紧凑簇：物体互相遮挡更多，自由空间被切得更碎
        "radius": 3.6, "elev_deg": 28.0,
        "objects": [
            ("floor", 1, (0.0, 0.0, 0.0), "plane", (0.0, 0.0, 0.0)),
            ("cube", 2, (-1.2, 0.6, 0.5), "cube", (-1.2, 0.6, 0.5)),
            ("sphere", 3, (0.9, -1.1, 0.7), "sphere", (0.9, -1.1, 0.7)),
            ("cylinder", 4, (1.6, 0.9, 0.6), "cylinder", (1.6, 0.9, 0.6)),
            ("torus", 5, (-0.6, -1.6, 0.45), "torus", (-0.1, -1.6, 0.45)),
        ],
    },
    "D": {  # 高遮挡：四个物体挤在 1.4 m 内互相挡住，相机压到 r=3.0
        "radius": 3.0, "elev_deg": 22.0,
        "objects": [
            ("floor", 1, (0.0, 0.0, 0.0), "plane", (0.0, 0.0, 0.0)),
            ("cube", 2, (-0.55, 0.35, 0.5), "cube", (-0.55, 0.35, 0.5)),
            ("sphere", 3, (0.45, -0.5, 0.7), "sphere", (0.45, -0.5, 0.7)),
            ("cylinder", 4, (0.35, 0.75, 0.6), "cylinder", (0.35, 0.75, 0.6)),
            ("torus", 5, (-0.35, -0.85, 0.45), "torus", (0.15, -0.85, 0.45)),
        ],
    },
    "C": {  # 稀疏大场景：相机更远、仰角更低，地面占比更大
        "radius": 5.0, "elev_deg": 14.0,
        "objects": [
            ("floor", 1, (0.0, 0.0, 0.0), "plane", (0.0, 0.0, 0.0)),
            ("cube", 2, (-2.0, 1.2, 0.5), "cube", (-2.0, 1.2, 0.5)),
            ("sphere", 3, (2.0, 1.0, 0.7), "sphere", (2.0, 1.0, 0.7)),
            ("cylinder", 4, (0.0, -1.8, 0.6), "cylinder", (0.0, -1.8, 0.6)),
            ("torus", 5, (1.8, -1.4, 0.45), "torus", (2.3, -1.4, 0.45)),
        ],
    },
}
SCENE_OBJECTS = LAYOUTS["A"]["objects"]  # 默认布局，main 里按 --layout 覆盖
SENSOR_WIDTH = 36.0
LENS_MM = 35.0
PASS_DIR: Path | None = None


def parse_args() -> argparse.Namespace:
    argv = sys.argv[sys.argv.index("--") + 1 :] if "--" in sys.argv else []
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True)
    ap.add_argument("--frames", type=int, default=16)
    ap.add_argument("--res", type=int, default=512)
    ap.add_argument("--samples", type=int, default=16)
    ap.add_argument("--device", choices=["CPU", "GPU"], default="GPU")
    ap.add_argument("--layout", choices=sorted(LAYOUTS), default="A")
    ap.add_argument("--objects-json", default=None,
                    help="外部物体清单（装配体用），给了就忽略 --layout 的物体表")
    ap.add_argument("--radius", type=float, default=None)   # None = 用布局默认
    ap.add_argument("--elev-deg", type=float, default=None)
    ap.add_argument("--elev-sweep", type=float, default=0.0, help="逐帧仰角正弦扫描幅度（度）；0=旧行为")
    # 外观域随机化（默认 None/1.0 = 与既有数据逐位一致）
    ap.add_argument("--light-az", type=float, default=None, help="主光方位角（度），给了才改位置")
    ap.add_argument("--light-el", type=float, default=None, help="主光仰角（度）")
    ap.add_argument("--light-energy", type=float, default=None)
    ap.add_argument("--world-strength", type=float, default=None)
    ap.add_argument("--hue", type=float, default=None, help="部件材质色相旋转（度）")
    ap.add_argument("--value", type=float, default=None, help="部件材质明度乘子")
    ap.add_argument("--color-random", action="store_true",
                    help="逐部件伪随机取色（与部件 id 解耦；默认关=旧行为 palette[idx%%6]）")
    ap.add_argument("--color-seed", type=int, default=1)
    return ap.parse_args(argv)


# ---------------------------------------------------------------- 场景搭建


def reset_scene() -> None:
    bpy.ops.wm.read_factory_settings(use_empty=True)


# 材质类 → PBR（颜色 + 金属度 + 粗糙度）；未知材料退回 None（用旧 palette 行为）
MATERIAL_PRESETS = {
    "wood":   ((0.45, 0.28, 0.15, 1.0), 0.0, 0.65),
    "metal":  ((0.72, 0.74, 0.78, 1.0), 0.9, 0.25),
    "stone":  ((0.50, 0.50, 0.48, 1.0), 0.0, 0.85),
    "fabric": ((0.30, 0.42, 0.72, 1.0), 0.0, 0.95),
}


def make_pbr(name: str, rgba, metallic: float, roughness: float) -> bpy.types.Material:
    mat = make_material(name, rgba)
    bsdf = mat.node_tree.nodes.get("Principled BSDF")
    if bsdf is not None:
        bsdf.inputs["Metallic"].default_value = metallic
        bsdf.inputs["Roughness"].default_value = roughness
    return mat


def make_material(name: str, rgba: tuple[float, float, float, float]) -> bpy.types.Material:
    mat = bpy.data.materials.new(name)
    mat.use_nodes = True
    bsdf = mat.node_tree.nodes["Principled BSDF"]
    bsdf.inputs["Base Color"].default_value = rgba
    bsdf.inputs["Roughness"].default_value = 0.55
    return mat


def checker_material(name: str) -> bpy.types.Material:
    mat = bpy.data.materials.new(name)
    mat.use_nodes = True
    nodes, links = mat.node_tree.nodes, mat.node_tree.links
    bsdf = nodes["Principled BSDF"]
    tex = nodes.new("ShaderNodeTexChecker")
    tex.inputs["Scale"].default_value = 6.0
    tex.inputs["Color1"].default_value = (0.72, 0.72, 0.70, 1.0)
    tex.inputs["Color2"].default_value = (0.28, 0.29, 0.31, 1.0)
    links.new(tex.outputs["Color"], bsdf.inputs["Base Color"])
    bsdf.inputs["Roughness"].default_value = 0.8
    return mat


def add_object(kind: str, name: str, center, index: int, scale=(1.0, 1.0, 1.0), rot=(0.0, 0.0, 0.0)) -> None:
    if kind == "plane":
        bpy.ops.mesh.primitive_plane_add(size=12.0, location=center)
    elif kind == "cube":
        bpy.ops.mesh.primitive_cube_add(size=1.0, location=center)
    elif kind == "sphere":
        bpy.ops.mesh.primitive_uv_sphere_add(radius=0.7, segments=48, ring_count=24, location=center)
    elif kind == "cylinder":
        bpy.ops.mesh.primitive_cylinder_add(radius=0.45, depth=1.2, vertices=48, location=center)
    elif kind == "part_cube":
        bpy.ops.mesh.primitive_cube_add(size=0.055, location=center)
    elif kind == "torus":
        bpy.ops.mesh.primitive_torus_add(
            major_radius=0.5, minor_radius=0.18, major_segments=48, minor_segments=16, location=center
        )
    else:  # pragma: no cover
        raise ValueError(kind)
    obj = bpy.context.active_object
    obj.name = name
    obj.pass_index = index
    # 逐物体缩放/旋转（默认 (1,1,1)/(0,0,0) ⇒ 与旧数据逐位一致）
    obj.scale = tuple(scale)
    obj.rotation_euler = tuple(math.radians(a) for a in rot)
    bpy.ops.object.shade_smooth()
    if kind == "plane":
        bpy.ops.object.shade_flat()


def build_scene(objects=None, look=None) -> None:
    objects = SCENE_OBJECTS if objects is None else objects
    look = look or {}
    reset_scene()
    scene = bpy.context.scene
    scene.unit_settings.system = "METRIC"

    for o in objects:
        name, index, center, kind = o[0], o[1], o[2], o[3]
        scale = o[5] if len(o) > 5 and o[5] is not None else (1.0, 1.0, 1.0)
        rot = o[6] if len(o) > 6 and o[6] is not None else (0.0, 0.0, 0.0)
        add_object(kind, name, center, index, scale, rot)

    colors = {
        "floor": None,
        "cube": (0.85, 0.25, 0.20, 1.0),
        "sphere": (0.20, 0.55, 0.85, 1.0),
        "cylinder": (0.25, 0.80, 0.35, 1.0),
        "torus": (0.90, 0.75, 0.15, 1.0),
    }
    palette = [
        (0.85, 0.25, 0.20, 1.0), (0.20, 0.55, 0.85, 1.0), (0.25, 0.80, 0.35, 1.0),
        (0.90, 0.75, 0.15, 1.0), (0.70, 0.40, 0.85, 1.0), (0.30, 0.85, 0.80, 1.0),
    ]
    # 外观域随机化：色相旋转 / 明度乘子（默认 None ⇒ 与既有数据一致）
    hue = look.get("hue")
    val = look.get("value")

    def tint(rgba):
        r, g, b, a = rgba
        if hue is not None:
            h, s, v = colorsys.rgb_to_hsv(r, g, b)
            r, g, b = colorsys.hsv_to_rgb((h + hue / 360.0) % 1.0, s, v)
        if val is not None:
            r, g, b = r * val, g * val, b * val
        return (min(1.0, r), min(1.0, g), min(1.0, b), a)

    for idx, (name, *_rest) in enumerate(objects):
        obj = bpy.data.objects[name]
        if name == "floor":
            obj.data.materials.append(checker_material("mat_floor"))
            continue
        # objects 元组：(name, pass_index, center, kind, probe, scale, rot, material)
        mat_name = _rest[6] if len(_rest) > 6 else None
        if mat_name in MATERIAL_PRESETS:
            base, metallic, rough = MATERIAL_PRESETS[mat_name]
            obj.data.materials.append(make_pbr(f"mat_{mat_name}_{name}", tint(base), metallic, rough))
            continue
        if look.get("color_random"):
            rgba = palette[random.Random(look["color_seed"] * 100003 + idx).randrange(len(palette))]
        else:
            rgba = colors.get(name, palette[idx % len(palette)])
        obj.data.materials.append(make_material(f"mat_{name}", tint(rgba)))

    # 灯光：面光 + 微弱环境光（确定性，无 HDRI）。给了 --light-az/--light-el 时按球面摆位并指向原点。
    light_data = bpy.data.lights.new("key", type="AREA")
    light_data.energy = 900.0 if look.get("light_energy") is None else look["light_energy"]
    light_data.size = 4.0
    light = bpy.data.objects.new("key", light_data)
    if look.get("light_az") is None and look.get("light_el") is None:
        light.location = (3.5, -3.0, 6.0)
        light.rotation_euler = (math.radians(35), 0.0, math.radians(45))
    else:
        az = math.radians(45.0 if look.get("light_az") is None else look["light_az"])
        el = math.radians(35.0 if look.get("light_el") is None else look["light_el"])
        radius = 7.5
        loc = Vector((
            radius * math.cos(el) * math.cos(az),
            radius * math.cos(el) * math.sin(az),
            radius * math.sin(el),
        ))
        light.location = loc
        light.rotation_euler = (-loc).to_track_quat("-Z", "Y").to_euler()
    bpy.context.collection.objects.link(light)

    world = bpy.data.worlds.new("World")
    world.use_nodes = True
    world.node_tree.nodes["Background"].inputs["Color"].default_value = (0.05, 0.06, 0.08, 1.0)
    strength = 0.35 if look.get("world_strength") is None else look["world_strength"]
    world.node_tree.nodes["Background"].inputs["Strength"].default_value = strength
    scene.world = world


def setup_camera(res: int) -> bpy.types.Object:
    scene = bpy.context.scene
    cam_data = bpy.data.cameras.new("cam")
    cam_data.lens = LENS_MM
    cam_data.sensor_width = SENSOR_WIDTH
    cam_data.sensor_fit = "HORIZONTAL"
    cam_data.clip_start = 0.05
    cam_data.clip_end = 100.0
    cam = bpy.data.objects.new("cam", cam_data)
    bpy.context.collection.objects.link(cam)
    scene.camera = cam
    scene.render.resolution_x = res
    scene.render.resolution_y = res
    scene.render.resolution_percentage = 100
    scene.render.pixel_aspect_x = 1.0
    scene.render.pixel_aspect_y = 1.0
    return cam


def setup_render(samples: int, device: str) -> None:
    scene = bpy.context.scene
    scene.render.engine = "CYCLES"
    scene.cycles.samples = samples
    scene.cycles.use_denoising = False
    scene.cycles.seed = 0
    scene.cycles.use_animated_seed = False
    scene.cycles.device = "GPU" if device == "GPU" else "CPU"
    if device == "GPU":
        prefs = bpy.context.preferences.addons["cycles"].preferences
        prefs.compute_device_type = "OPTIX"
        prefs.get_devices()
        for dev in prefs.devices:
            dev.use = dev.type == "OPTIX"
    scene.render.film_transparent = False
    scene.render.image_settings.file_format = "PNG"
    scene.render.image_settings.color_mode = "RGB"
    scene.render.image_settings.color_depth = "8"
    scene.view_settings.view_transform = "Standard"
    scene.view_settings.look = "None"

    vl = scene.view_layers[0]
    vl.use_pass_z = True
    vl.use_pass_normal = True
    vl.use_pass_object_index = True
    vl.use_pass_combined = True

    # 4.2 的 Render Result 不再暴露 layers/passes；用合成器把每个 pass 写成
    # 单层 32-bit EXR，再由 Blender 自己读回成 npy（不引入 OpenEXR 依赖）。
    scene.use_nodes = True
    tree = scene.node_tree
    tree.nodes.clear()
    layers = tree.nodes.new("CompositorNodeRLayers")
    for name, socket in (("depth", "Depth"), ("normal", "Normal"), ("seg", "IndexOB")):
        node = tree.nodes.new("CompositorNodeOutputFile")
        node.base_path = str(PASS_DIR)
        node.format.file_format = "OPEN_EXR"
        node.format.color_depth = "32"
        node.file_slots[0].path = name + "_"
        tree.links.new(layers.outputs[socket], node.inputs[0])


# ---------------------------------------------------------------- 相机轨迹


def camera_pose(frame: int, frames: int, radius: float, elev_deg: float, elev_sweep: float = 0.0):
    """固定半径圆轨；仰角可加正弦扫描（`elev_sweep` 度，0 = 旧行为）。"""
    theta = 2.0 * math.pi * frame / frames
    elev = math.radians(elev_deg + elev_sweep * math.sin(2.0 * math.pi * frame / max(1, frames)))
    target = Vector((0.2, 0.2, 0.6))
    loc = Vector(
        (
            target.x + radius * math.cos(elev) * math.cos(theta),
            target.y + radius * math.cos(elev) * math.sin(theta),
            target.z + radius * math.sin(elev),
        )
    )
    return loc, target


def look_at(cam: bpy.types.Object, loc: Vector, target: Vector) -> None:
    cam.location = loc
    direction = target - loc
    cam.rotation_euler = direction.to_track_quat("-Z", "Y").to_euler()


def intrinsics(res: int) -> list[list[float]]:
    fx = LENS_MM / SENSOR_WIDTH * res
    fy = fx
    # Blender 的像素中心在 (res-1)/2；用 res/2 会带来 ~6 mm 的地面深度误差（实测）。
    cx = cy = (res - 1) / 2.0
    return [[fx, 0.0, cx], [0.0, fy, cy], [0.0, 0.0, 1.0]]


def matrix_to_list(mat) -> list[list[float]]:
    return [[float(mat[r][c]) for c in range(4)] for r in range(4)]


# ---------------------------------------------------------------- 读 pass


def read_pass(name: str, frame: int, res: int) -> np.ndarray:
    """读回合成器写的 EXR，翻成图像坐标（Blender 像素原点在左下）。"""
    path = PASS_DIR / f"{name}_{frame:04d}.exr"
    image = bpy.data.images.load(str(path))
    image.colorspace_settings.name = "Non-Color"
    pixels = np.asarray(image.pixels[:], dtype=np.float32).reshape(res, res, 4)[::-1]
    bpy.data.images.remove(image)
    return pixels


def render_frame(out: Path, frame: int, res: int) -> dict:
    scene = bpy.context.scene
    scene.frame_set(frame)
    scene.render.filepath = str(out / "rgb" / f"{frame:06d}.png")
    t0 = time.perf_counter()
    bpy.ops.render.render(write_still=True)
    elapsed = time.perf_counter() - t0

    depth = read_pass("depth", frame, res)[..., 0]
    normal = read_pass("normal", frame, res)[..., :3]
    seg = read_pass("seg", frame, res)[..., 0]

    np.save(out / "depth" / f"{frame:06d}.npy", depth.astype(np.float32))
    np.save(out / "normal" / f"{frame:06d}.npy", normal.astype(np.float32))
    np.save(out / "seg" / f"{frame:06d}.npy", np.rint(seg).astype(np.int32))

    cam = scene.camera
    t_wc = cam.matrix_world.copy()
    t_cw = t_wc.inverted()
    # 相机在渲染时已经摆好（可能带仰角扫描），meta 必须记录"真实"位姿，不能重算
    loc = t_wc.translation.copy()
    target = loc + (t_wc.to_3x3() @ Vector((0.0, 0.0, -1.0)))
    meta = {
        "frame": frame,
        "res": res,
        "rgb": f"rgb/{frame:06d}.png",
        "depth": f"depth/{frame:06d}.npy",
        "normal": f"normal/{frame:06d}.npy",
        "seg": f"seg/{frame:06d}.npy",
        "K": intrinsics(res),
        "T_wc": matrix_to_list(t_wc),
        "T_cw": matrix_to_list(t_cw),
        "camera_location": list(loc),
        "look_at": list(target),
        "lens_mm": LENS_MM,
        "sensor_width_mm": SENSOR_WIDTH,
        "near": cam.data.clip_start,
        "far": cam.data.clip_end,
        "render_seconds": elapsed,
    }
    (out / "meta" / f"{frame:06d}.json").write_text(json.dumps(meta, indent=2), encoding="utf-8")
    return meta


# ---------------------------------------------------------------- main


def main() -> None:
    global SCENE_OBJECTS
    args = parse_args()
    layout = LAYOUTS[args.layout]
    if args.objects_json:
        raw = json.loads(Path(args.objects_json).read_text(encoding="utf-8"))
        layout = {
            "radius": args.radius if args.radius is not None else 3.4,
            "elev_deg": args.elev_deg if args.elev_deg is not None else 12.0,
            "objects": [
                (
                    o["name"],
                    int(o["pass_index"]),
                    tuple(o["center"]),
                    o["kind"],
                    tuple(o.get("probe", o["center"])),
                    tuple(o.get("scale", (1.0, 1.0, 1.0)))
                    if isinstance(o.get("scale"), (list, tuple))
                    else (float(o.get("scale", 1.0)),) * 3,
                    tuple(o.get("rot", (0.0, 0.0, 0.0))),
                    o.get("material"),
                )
                for o in raw
            ],
        }
    SCENE_OBJECTS = layout["objects"]
    if args.radius is None:
        args.radius = layout["radius"]
    if args.elev_deg is None:
        args.elev_deg = layout["elev_deg"]
    out = Path(args.out)
    for sub in ("rgb", "depth", "normal", "seg", "meta"):
        (out / sub).mkdir(parents=True, exist_ok=True)
    global PASS_DIR
    PASS_DIR = out / "passes"
    PASS_DIR.mkdir(parents=True, exist_ok=True)

    look = {
        "color_random": args.color_random,
        "light_az": args.light_az,
        "light_el": args.light_el,
        "light_energy": args.light_energy,
        "world_strength": args.world_strength,
        "hue": args.hue,
        "value": args.value,
        "color_seed": args.color_seed,
    }
    build_scene(SCENE_OBJECTS, look)
    cam = setup_camera(args.res)
    setup_render(args.samples, args.device)
    scene = bpy.context.scene
    scene["radius"] = args.radius
    scene["elev_deg"] = args.elev_deg
    scene.frame_start = 0
    scene.frame_end = args.frames - 1

    frames_meta = []
    for frame in range(args.frames):
        loc, target = camera_pose(
            frame, args.frames, args.radius, args.elev_deg, args.elev_sweep
        )
        look_at(cam, loc, target)
        bpy.context.view_layer.update()
        meta = render_frame(out, frame, args.res)
        frames_meta.append(meta)
        print(
            f"FRAME {frame:03d} {meta['render_seconds']:.2f}s "
            f"seg_ids={sorted(set(np.load(out / 'seg' / f'{frame:06d}.npy').ravel().tolist()))}",
            flush=True,
        )

    manifest = {
        "blender": bpy.app.version_string,
        "scene": [
            {
                "name": o[0],
                "pass_index": o[1],
                "center": o[2],
                "kind": o[3],
                "probe": o[4],
                "scale": (o[5] if len(o) > 5 else (1.0, 1.0, 1.0)),
                "rot": (o[6] if len(o) > 6 else (0.0, 0.0, 0.0)),
                "material": (o[7] if len(o) > 7 else None),
            }
            for o in SCENE_OBJECTS
        ],
        "config": {
            "frames": args.frames,
            "res": args.res,
            "samples": args.samples,
            "device": args.device,
            "layout": args.layout,
            "radius": args.radius,
            "elev_deg": args.elev_deg,
            "lens_mm": LENS_MM,
            "sensor_width_mm": SENSOR_WIDTH,
        },
        "frames_meta": frames_meta,
    }
    (out / "manifest.json").write_text(json.dumps(manifest, indent=2), encoding="utf-8")
    total = sum(m["render_seconds"] for m in frames_meta)
    print(f"DONE frames={args.frames} total={total:.2f}s mean={total / args.frames:.3f}s/frame", flush=True)


if __name__ == "__main__":
    main()

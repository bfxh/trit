//! GLB（glTF 2.0 二进制）打包：把一个场景里的对象网格 + 图集打成一个 `.glb`。
//!
//! 每个对象一个 mesh / material / texture / node；PNG 直接嵌进 BIN chunk，
//! 节点只带布局变换（绕 Y 旋转 + 平移），引擎（Blender/Unity/three.js）可直接导入。
//!
//! 顶点按 `(顶点号, uv)` 去重：同一 chart 内共享角点的 UV 相同，跨 chart 才拆开。

use std::collections::HashMap;

use crate::mesh::Mesh;

mod manifest;
mod scene;
pub use scene::{Loaded, load_scene};

/// 每三角形三个角的 UV（与 `mesh.tris` 一一对应）。
pub type UvTris = Vec<[[f32; 2]; 3]>;

pub struct Part {
    pub name: String,
    pub mesh: Mesh,
    /// 与 `mesh.tris` 一一对应的三角角 UV。
    pub uv: UvTris,
    pub png: Vec<u8>,
    pub translation: [f32; 3],
    /// 四元数 `(x, y, z, w)`。
    pub rotation: [f32; 4],
}

/// 逐顶点法线（面积加权；退化点回落到 +Y）。
pub fn vertex_normals(m: &Mesh) -> Vec<[f32; 3]> {
    let mut acc = vec![[0f64; 3]; m.verts.len()];
    for t in &m.tris {
        let (a, b, c) = (
            m.verts[t[0] as usize],
            m.verts[t[1] as usize],
            m.verts[t[2] as usize],
        );
        let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let n = [
            e1[1] * e2[2] - e1[2] * e2[1],
            e1[2] * e2[0] - e1[0] * e2[2],
            e1[0] * e2[1] - e1[1] * e2[0],
        ];
        for k in 0..3 {
            for (acc_cell, &nd) in acc[t[k] as usize].iter_mut().zip(n.iter()) {
                *acc_cell += nd as f64;
            }
        }
    }
    acc.into_iter()
        .map(|v| {
            let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            if l < 1e-20 {
                [0.0, 1.0, 0.0]
            } else {
                [(v[0] / l) as f32, (v[1] / l) as f32, (v[2] / l) as f32]
            }
        })
        .collect()
}

fn push_view(
    bin: &mut Vec<u8>,
    views: &mut Vec<(usize, usize, Option<u32>)>,
    data: &[u8],
    target: Option<u32>,
) -> usize {
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }
    let off = bin.len();
    bin.extend_from_slice(data);
    views.push((off, data.len(), target));
    views.len() - 1
}

fn f32s(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn accessor_time(acc: &mut Vec<String>, view: usize, count: usize, t0: f32, t1: f32) -> usize {
    acc.push(format!(
        "{{\"bufferView\": {view}, \"componentType\": 5126, \"count\": {count}, \"type\": \"SCALAR\", \"min\": [{t0}], \"max\": [{t1}]}}"
    ));
    acc.len() - 1
}

fn accessor(
    acc: &mut Vec<String>,
    view: usize,
    count: usize,
    ty: &str,
    component: u32,
    minmax: Option<([f32; 3], [f32; 3])>,
) -> usize {
    let mut s = format!(
        "{{\"bufferView\": {view}, \"componentType\": {component}, \"count\": {count}, \"type\": \"{ty}\""
    );
    if let Some((lo, hi)) = minmax {
        s.push_str(&format!(
            ", \"min\": [{}, {}, {}], \"max\": [{}, {}, {}]",
            lo[0], lo[1], lo[2], hi[0], hi[1], hi[2]
        ));
    }
    s.push('}');
    acc.push(s);
    acc.len() - 1
}

/// 生成 GLB 字节（确定性：同样输入逐字节相同）。
pub fn build(parts: &[Part], generator: &str) -> Vec<u8> {
    build_with(parts, generator, None)
}

/// 同 [`build`]，`spin_seconds > 0` 时额外写一条绕 Y 轴 360° 的转台动画：
/// 所有对象挂在一个 `turntable` 根节点下，动画只转根节点（引擎里直接播放）。
pub fn build_with(parts: &[Part], generator: &str, spin_seconds: Option<f32>) -> Vec<u8> {
    let mut bin = Vec::new();
    let mut views: Vec<(usize, usize, Option<u32>)> = Vec::new();
    let (mut accessors, mut meshes, mut materials, mut textures, mut images, mut nodes) = (
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );

    for p in parts {
        let normals = vertex_normals(&p.mesh);
        let mut index: HashMap<(u32, u32, u32), u32> = HashMap::new();
        let (mut pos, mut nor, mut tex, mut idx) = (
            Vec::<f32>::new(),
            Vec::<f32>::new(),
            Vec::<f32>::new(),
            Vec::<u32>::new(),
        );
        let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
        for (ti, t) in p.mesh.tris.iter().enumerate() {
            for (k, &vid) in t.iter().enumerate() {
                let (u, v) = (p.uv[ti][k][0], p.uv[ti][k][1]);
                let key = (vid, u.to_bits(), v.to_bits());
                let id = *index.entry(key).or_insert_with(|| {
                    let q = p.mesh.verts[vid as usize];
                    pos.extend_from_slice(&[q[0], q[1], q[2]]);
                    let n = normals[vid as usize];
                    nor.extend_from_slice(&[n[0], n[1], n[2]]);
                    tex.extend_from_slice(&[u, v]);
                    for d in 0..3 {
                        lo[d] = lo[d].min(q[d]);
                        hi[d] = hi[d].max(q[d]);
                    }
                    (pos.len() / 3 - 1) as u32
                });
                idx.push(id);
            }
        }
        let n = pos.len() / 3;
        if n == 0 || idx.is_empty() {
            continue;
        }
        let vp = push_view(&mut bin, &mut views, &f32s(&pos), Some(34962));
        let ap = accessor(&mut accessors, vp, n, "VEC3", 5126, Some((lo, hi)));
        let vn = push_view(&mut bin, &mut views, &f32s(&nor), Some(34962));
        let an = accessor(&mut accessors, vn, n, "VEC3", 5126, None);
        let vt = push_view(&mut bin, &mut views, &f32s(&tex), Some(34962));
        let at = accessor(&mut accessors, vt, n, "VEC2", 5126, None);
        let ib: Vec<u8> = idx.iter().flat_map(|x| x.to_le_bytes()).collect();
        let vi = push_view(&mut bin, &mut views, &ib, Some(34963));
        let ai = accessor(&mut accessors, vi, idx.len(), "SCALAR", 5125, None);
        // PNG 作为 image 的 bufferView
        let vpng = push_view(&mut bin, &mut views, &p.png, None);
        let mi = meshes.len();
        meshes.push(format!(
            "{{\"name\": {}, \"primitives\": [{{\"attributes\": {{\"POSITION\": {ap}, \"NORMAL\": {an}, \"TEXCOORD_0\": {at}}}, \"indices\": {ai}, \"material\": {mi}}}]}}",
            crate::partsrun::json_str(&p.name)
        ));
        let ti = textures.len();
        textures.push(format!("{{\"source\": {ti}, \"sampler\": 0}}"));
        images.push(format!(
            "{{\"bufferView\": {vpng}, \"mimeType\": \"image/png\"}}"
        ));
        materials.push(format!(
            "{{\"name\": {}, \"pbrMetallicRoughness\": {{\"baseColorTexture\": {{\"index\": {ti}}}, \"metallicFactor\": 0.0, \"roughnessFactor\": 1.0}}, \"doubleSided\": true}}",
            crate::partsrun::json_str(&p.name)
        ));
        nodes.push(format!(
            "{{\"name\": {}, \"mesh\": {mi}, \"translation\": [{}, {}, {}], \"rotation\": [{}, {}, {}, {}]}}",
            crate::partsrun::json_str(&p.name),
            p.translation[0],
            p.translation[1],
            p.translation[2],
            p.rotation[0],
            p.rotation[1],
            p.rotation[2],
            p.rotation[3]
        ));
    }

    let mut anim_suffix = String::new();
    if let Some(sec) = spin_seconds {
        let t = sec.max(0.1);
        let (times, quats) = turntable_keys(t);
        let vt = push_view(&mut bin, &mut views, &f32s(&times), None);
        let at = accessor_time(&mut accessors, vt, 5, 0.0, t);
        let vq = push_view(&mut bin, &mut views, &f32s(&quats), None);
        let aq = accessor(&mut accessors, vq, 5, "VEC4", 5126, None);
        anim_suffix = format!(
            ", \"animations\": [{{\"name\": \"turntable\", \"samplers\": [{{\"input\": {at}, \"output\": {aq}, \"interpolation\": \"LINEAR\"}}], \"channels\": [{{\"sampler\": 0, \"target\": {{\"node\": 0, \"path\": \"rotation\"}}}}]}}]"
        );
        let children: Vec<String> = (1..=nodes.len()).map(|i| i.to_string()).collect();
        nodes.insert(
            0,
            format!(
                "{{\"name\": \"turntable\", \"children\": [{}]}}",
                children.join(", ")
            ),
        );
    }

    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }
    let scene_nodes: Vec<String> = if spin_seconds.is_some() {
        vec!["0".into()]
    } else {
        (0..nodes.len()).map(|i| i.to_string()).collect()
    };
    let views_json: Vec<String> = views
        .iter()
        .map(|(o, l, t)| match t {
            Some(t) => format!(
                "{{\"buffer\": 0, \"byteOffset\": {o}, \"byteLength\": {l}, \"target\": {t}}}"
            ),
            None => format!("{{\"buffer\": 0, \"byteOffset\": {o}, \"byteLength\": {l}}}"),
        })
        .collect();
    let mut json = format!(
        "{{\"asset\": {{\"version\": \"2.0\", \"generator\": {}}}, \"scene\": 0, \"scenes\": [{{\"nodes\": [{}]}}], \"nodes\": [{}], \"meshes\": [{}], \"materials\": [{}], \"textures\": [{}], \"images\": [{}], \"samplers\": [{{\"magFilter\": 9729, \"minFilter\": 9987, \"wrapS\": 10497, \"wrapT\": 10497}}], \"accessors\": [{}], \"bufferViews\": [{}], \"buffers\": [{{\"byteLength\": {}}}]",
        crate::partsrun::json_str(generator),
        scene_nodes.join(", "),
        nodes.join(", "),
        meshes.join(", "),
        materials.join(", "),
        textures.join(", "),
        images.join(", "),
        accessors.join(", "),
        views_json.join(", "),
        bin.len()
    );
    json.push_str(&anim_suffix);
    json.push('}');
    let mut jb = json.into_bytes();
    while !jb.len().is_multiple_of(4) {
        jb.push(b' ');
    }
    let total = glb_total(jb.len(), bin.len()) as usize;
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(jb.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&jb);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(&bin);
    out
}

/// GLB 头 12 + JSON 块头 8 + BIN 块头 8 = 28。三个长度字段都是 u32：
/// 总长一旦超过上限，`as u32` 会静默截断，写出的文件自己的 verify 判红。
fn glb_total(jl: usize, bl: usize) -> u32 {
    let total = jl
        .checked_add(bl)
        .and_then(|n| n.checked_add(28))
        .expect("GLB 总长超过 usize 上限");
    u32::try_from(total).expect("GLB 总长超过 u32 上限")
}

/// 转台动画的 5 个关键帧：时间 0..T 等分，四元数 0/90/180/270/360°（绕 Y）。
pub fn turntable_keys(seconds: f32) -> (Vec<f32>, Vec<f32>) {
    let t = seconds.max(0.1);
    let times: Vec<f32> = (0..5).map(|k| t * k as f32 / 4.0).collect();
    let mut quats: Vec<f32> = Vec::with_capacity(20);
    for k in 0..5 {
        let th = k as f32 * std::f32::consts::FRAC_PI_2;
        quats.extend_from_slice(&[0.0, (th / 2.0).sin(), 0.0, (th / 2.0).cos()]);
    }
    (times, quats)
}

/// 布局 → 节点 TRS：绕包围盒中心（xz）旋转 yaw，再平移，等价于
/// `world = R·p + T`（glTF 节点只支持绕原点旋转，所以 T 里含 `c − R·c`）。
pub fn node_trs(bmin: [f64; 3], bmax: [f64; 3], yaw_deg: u32, t: [f64; 3]) -> ([f32; 3], [f32; 4]) {
    let (cx, cz) = ((bmin[0] + bmax[0]) / 2.0, (bmin[2] + bmax[2]) / 2.0);
    let th = f64::from(yaw_deg % 360).to_radians();
    let (s, c) = th.sin_cos();
    let rc = [cx * c + cz * s, -cx * s + cz * c];
    let tr = [cx + t[0] - rc[0], t[1], cz + t[2] - rc[1]];
    let (hs, hc) = (th / 2.0).sin_cos();
    (
        [tr[0] as f32, tr[1] as f32, tr[2] as f32],
        [0.0, hs as f32, 0.0, hc as f32],
    )
}

/// 解析本仓 `uv::write_obj` 写出的 OBJ（v / vt / f 三样）。
mod obj;
pub use obj::parse_obj;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glb_total_rejects_lengths_beyond_u32() {
        // u32::MAX 字节的 JSON 加上 28 字节头就装不进 u32 了：必须拒，不许截断。
        let r = std::panic::catch_unwind(|| glb_total(u32::MAX as usize, 0));
        assert!(
            r.is_err(),
            "总长超 u32 必须拒，静默截断会写出 verify 判红的文件"
        );
        assert_eq!(glb_total(0, 0), 28, "空场景正好是三个块头");
    }

    fn tri_mesh() -> Mesh {
        Mesh {
            verts: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            tris: vec![[0, 1, 2]],
        }
    }

    fn part(name: &str) -> Part {
        Part {
            name: name.into(),
            mesh: tri_mesh(),
            uv: vec![[[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]],
            png: vec![0x89, b'P', b'N', b'G'],
            translation: [0.5, 0.0, -1.5],
            rotation: [
                0.0,
                std::f32::consts::FRAC_1_SQRT_2,
                0.0,
                std::f32::consts::FRAC_1_SQRT_2,
            ],
        }
    }

    #[test]
    fn glb_has_header_chunks_and_expected_json() {
        let b = build(&[part("obj-1")], "trit-parts");
        assert_eq!(&b[0..4], b"glTF");
        let ver = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
        let len = u32::from_le_bytes([b[8], b[9], b[10], b[11]]) as usize;
        assert_eq!(ver, 2);
        assert_eq!(len, b.len());
        let jl = u32::from_le_bytes([b[12], b[13], b[14], b[15]]) as usize;
        assert_eq!(&b[16..20], b"JSON");
        let json = std::str::from_utf8(&b[20..20 + jl]).unwrap();
        assert!(json.contains("\"meshes\": [{"));
        assert!(json.contains("\"images\": [{"));
        assert!(json.contains("\"translation\": [0.5, 0, -1.5]"));
        assert!(json.contains("\"TEXCOORD_0\""));
        let bo = 20 + jl;
        let bl = u32::from_le_bytes([b[bo], b[bo + 1], b[bo + 2], b[bo + 3]]) as usize;
        assert_eq!(&b[bo + 4..bo + 8], b"BIN\0");
        assert_eq!(bo + 8 + bl, b.len());
        assert!(b.windows(4).any(|w| w == [0x89, b'P', b'N', b'G']));
    }

    #[test]
    fn build_is_byte_deterministic_and_dedups_vertices() {
        let a = build(&[part("obj-1"), part("obj-2")], "trit-parts");
        let c = build(&[part("obj-1"), part("obj-2")], "trit-parts");
        assert_eq!(a, c);
        // 单三角形 3 个角点 → 3 个顶点（不重复）
        let json = String::from_utf8_lossy(&a);
        assert!(json.contains("\"count\": 3"));
    }

    #[test]
    fn normals_are_unit_and_parse_obj_roundtrips() {
        // 故意用非单位面积的三角形：面积加权法线模长 = 2·面积 = 6，归一化才应为 1
        let m = Mesh {
            verts: vec![[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 3.0, 0.0]],
            tris: vec![[0, 1, 2]],
        };
        let n = vertex_normals(&m);
        assert_eq!(n.len(), 3);
        for q in &n {
            let l = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2]).sqrt();
            assert!((l - 1.0).abs() < 1e-6);
            assert!((q[2] - 1.0).abs() < 1e-6, "平面朝 +Z");
        }
        let obj = "v 0 0 0\nv 1 0 0\nv 0 1 0\nvt 0 0\nvt 1 0\nvt 0 1\nusemtl tex\nf 1/1 2/2 3/3\n";
        let (m2, uv) = parse_obj(obj).unwrap();
        assert_eq!((m2.verts.len(), m2.tris.len()), (3, 1));
        assert_eq!(uv[0][1], [1.0, 0.0]);
        assert!(parse_obj("v 0 0 0\n").is_err());
    }

    /// OBJ 的顶点号/UV 号从 1 开始。`0` 会让 `0 - 1` 在 u32 上溢出，越界号则直接
    /// 打穿 `uvs[ti]` 的索引 —— 两者都只能返回 Err，不能把进程带走。
    #[test]
    fn obj_indices_out_of_range_are_errors_not_panics() {
        let base = "v 0 0 0\nv 1 0 0\nv 0 1 0\n";

        assert!(parse_obj(&format!("{base}f 0 1 2\n")).is_err(), "0 号顶点");

        assert!(
            parse_obj(&format!("{base}vt 0 0\nf 1/9 2/9 3/9\n")).is_err(),
            "UV 号越界"
        );

        assert!(
            parse_obj(&format!("{base}vt 0 0\nf 1/1 2/1 9/1\n")).is_err(),
            "顶点号越界"
        );

        assert!(
            parse_obj(&format!("{base}f 1 2 3\n")).is_err(),
            "没有 vt 时不能静默越界"
        );

        let ok = format!("{base}vt 0 0\nvt 1 0\nvt 0 1\nf 1/1 2/2 3/3\n");
        let (m, uv) = parse_obj(&ok).expect("合法 OBJ 应通过");
        assert_eq!((m.verts.len(), m.tris.len(), uv.len()), (3, 1, 1));
    }

    #[test]
    fn turntable_animation_adds_root_and_keyframes() {
        let a = build(&[part("obj-1")], "t");
        let b = build_with(&[part("obj-1")], "t", Some(2.0));
        let (ja, _) = chunks_of(&a);
        let (jb, _) = chunks_of(&b);
        assert!(!ja.contains("\"animations\""));
        assert!(jb.contains("\"animations\": [{"));
        assert!(jb.contains("\"name\": \"turntable\""));
        assert!(jb.contains("\"path\": \"rotation\""));
        assert_eq!(jb.matches("\"mesh\": 0").count(), 1);
        // 根节点把唯一对象挂成 child 1；obj-1 出现在 node/mesh/material 三处
        assert!(jb.contains("\"children\": [1]"));
        assert_eq!(jb.matches("\"name\": \"obj-1\"").count(), 3);
        let c = build_with(&[part("obj-1")], "t", Some(2.0));
        assert_eq!(b, c, "动画输出也必须逐字节确定");
    }

    fn chunks_of(b: &[u8]) -> (String, &[u8]) {
        let jl = u32::from_le_bytes([b[12], b[13], b[14], b[15]]) as usize;
        let json = String::from_utf8_lossy(&b[20..20 + jl]).to_string();
        let bo = 20 + jl;
        let bl = u32::from_le_bytes([b[bo], b[bo + 1], b[bo + 2], b[bo + 3]]) as usize;
        (json, &b[bo + 8..bo + 8 + bl])
    }

    #[test]
    fn turntable_keys_are_quarter_turns() {
        let (t, q) = turntable_keys(4.0);
        assert_eq!(t, vec![0.0, 1.0, 2.0, 3.0, 4.0]);
        let h = std::f32::consts::FRAC_1_SQRT_2;
        // 90°：(0, √½, 0, √½)；180°：(0,1,0,0)；360°：(0,0,0,-1)
        assert!((q[4] - 0.0).abs() < 1e-6 && (q[5] - h).abs() < 1e-6);
        assert!((q[6] - 0.0).abs() < 1e-6 && (q[7] - h).abs() < 1e-6);
        assert!((q[9] - 1.0).abs() < 1e-6 && q[10].abs() < 1e-6);
        assert!((q[19] + 1.0).abs() < 1e-6, "360° = (0,0,0,-1)");
        assert_eq!(t.len(), 5);
    }

    #[test]
    fn node_trs_matches_layout_transform() {
        use crate::splat::{Item, Placement, transform_point};
        let it = Item {
            id: 1,
            bbox_min: [0.0, 0.0, 0.0],
            bbox_max: [2.0, 1.0, 1.0],
        };
        let pl = Placement {
            id: 1,
            yaw: 90,
            translate: [0.3, -0.2, 1.1],
            bbox_min: [0.0, 0.0, 0.0],
            bbox_max: [0.0, 0.0, 0.0],
        };
        let (t, r) = node_trs(it.bbox_min, it.bbox_max, pl.yaw, pl.translate);
        let th = (90f64).to_radians();
        let (hs, hc) = (th / 2.0).sin_cos();
        assert!((r[1] - hs as f32).abs() < 1e-6 && (r[3] - hc as f32).abs() < 1e-6);
        let (s, c) = th.sin_cos();
        for p in [[0.0, 0.0, 0.0], [2.0, 1.0, 1.0], [0.7, 0.2, 0.9]] {
            let want = transform_point(p, &it, &pl);
            let got = [
                (p[0] as f64 * c + p[2] as f64 * s + t[0] as f64) as f32,
                p[1] + t[1],
                (-p[0] as f64 * s + p[2] as f64 * c + t[2] as f64) as f32,
            ];
            for k in 0..3 {
                assert!((want[k] - got[k]).abs() < 1e-5, "{want:?} vs {got:?}");
            }
        }
    }

    #[test]
    fn load_scene_reads_bundle() {
        let t = std::env::temp_dir().join(format!("trit_glb_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&t);
        let d = t.join("objects-mesh");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            t.join("objects.manifest.json"),
            "{\"objects\": [{\"id\": 1, \"bbox_min\": [0,0,0], \"bbox_max\": [2,1,1]}]}",
        )
        .unwrap();
        std::fs::write(
            t.join("compose.manifest.json"),
            "{\"layout\": [{\"id\": 1, \"yaw\": 90, \"translate\": [0.3, 0, 0], \"bbox_min\": [0,0,0], \"bbox_max\": [0,0,0]}]}",
        )
        .unwrap();
        std::fs::write(
            d.join("objects-mesh.manifest.json"),
            "{\"inputs\": {\"objects_manifest\": {\"path\": \"../objects.manifest.json\", \"sha256\": \"x\"}, \"layout\": {\"path\": \"../compose.manifest.json\", \"sha256\": \"y\"}}, \"objects\": [{\"id\": 1, \"obj\": \"obj-1.obj\", \"png\": \"obj-1.png\"}]}",
        )
        .unwrap();
        std::fs::write(
            d.join("obj-1.obj"),
            "v 0 0 0
v 2 0 0
v 0 1 0
vt 0 0
vt 1 0
vt 0 1
f 1/1 2/2 3/3
",
        )
        .unwrap();
        std::fs::write(d.join("obj-1.png"), [0x89, b'P', b'N', b'G']).unwrap();
        let l = load_scene(&d, None).unwrap();
        assert_eq!((l.parts.len(), l.verts, l.tris), (1, 3, 1));
        let want = node_trs([0.0, 0.0, 0.0], [2.0, 1.0, 1.0], 90, [0.3, 0.0, 0.0]);
        assert_eq!(l.parts[0].translation, want.0);
        assert_eq!(l.parts[0].rotation, want.1);
        let _ = std::fs::remove_dir_all(&t);
    }

    #[test]
    fn load_scene_rejects_manifest_paths_outside_the_package() {
        let d = std::env::temp_dir().join(format!("trit-glb-escape-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join("objects-mesh.manifest.json"),
            "{\"inputs\": {\"objects_manifest\": {\"path\": \"../../../etc/passwd\"}}}",
        )
        .unwrap();
        let err = match load_scene(&d, None) {
            Ok(_) => panic!("越界路径必须被拒"),
            Err(e) => e,
        };
        assert!(err.contains("越出包目录"), "{err}");
        let _ = std::fs::remove_dir_all(&d);
    }
}

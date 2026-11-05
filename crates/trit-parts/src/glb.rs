//! GLB（glTF 2.0 二进制）打包：把一个场景里的对象网格 + 图集打成一个 `.glb`。
//!
//! 每个对象一个 mesh / material / texture / node；PNG 直接嵌进 BIN chunk，
//! 节点只带布局变换（绕 Y 旋转 + 平移），引擎（Blender/Unity/three.js）可直接导入。
//!
//! 顶点按 `(顶点号, uv)` 去重：同一 chart 内共享角点的 UV 相同，跨 chart 才拆开。

use std::collections::HashMap;

use crate::mesh::Mesh;

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
            for d in 0..3 {
                acc[t[k] as usize][d] += n[d] as f64;
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
    let mut bin = Vec::new();
    let mut views: Vec<(usize, usize, Option<u32>)> = Vec::new();
    let mut accessors: Vec<String> = Vec::new();
    let mut meshes: Vec<String> = Vec::new();
    let mut materials: Vec<String> = Vec::new();
    let mut textures: Vec<String> = Vec::new();
    let mut images: Vec<String> = Vec::new();
    let mut nodes: Vec<String> = Vec::new();

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

    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }
    let scene_nodes: Vec<String> = (0..nodes.len()).map(|i| i.to_string()).collect();
    let views_json: Vec<String> = views
        .iter()
        .map(|(o, l, t)| match t {
            Some(t) => format!(
                "{{\"buffer\": 0, \"byteOffset\": {o}, \"byteLength\": {l}, \"target\": {t}}}"
            ),
            None => format!("{{\"buffer\": 0, \"byteOffset\": {o}, \"byteLength\": {l}}}"),
        })
        .collect();
    let json = format!(
        "{{\"asset\": {{\"version\": \"2.0\", \"generator\": {}}}, \"scene\": 0, \"scenes\": [{{\"nodes\": [{}]}}], \"nodes\": [{}], \"meshes\": [{}], \"materials\": [{}], \"textures\": [{}], \"images\": [{}], \"samplers\": [{{\"magFilter\": 9729, \"minFilter\": 9987, \"wrapS\": 10497, \"wrapT\": 10497}}], \"accessors\": [{}], \"bufferViews\": [{}], \"buffers\": [{{\"byteLength\": {}}}]}}",
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
    let mut jb = json.into_bytes();
    while !jb.len().is_multiple_of(4) {
        jb.push(b' ');
    }
    let total = 12 + 8 + jb.len() + 8 + bin.len();
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

fn field(txt: &str, key: &str) -> Option<String> {
    let pat = format!("\"{key}\": \"");
    let i = txt.find(&pat)? + pat.len();
    let rest = &txt[i..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn number(txt: &str, key: &str) -> Option<f64> {
    let pat = format!("\"{key}\": ");
    let i = txt.find(&pat)? + pat.len();
    let rest = &txt[i..];
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == 'e' || c == '+'))
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

fn nums(txt: &str, key: &str) -> Vec<f64> {
    let pat = format!("\"{key}\": [");
    let Some(i) = txt.find(&pat) else {
        return Vec::new();
    };
    let rest = &txt[i + pat.len()..];
    let end = rest.find(']').unwrap_or(rest.len());
    rest[..end]
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect()
}

fn object_block<'a>(txt: &'a str, key: &str) -> Option<&'a str> {
    let pat = format!("\"{key}\": {{");
    let i = txt.find(&pat)? + pat.len();
    let rest = &txt[i..];
    let mut depth = 1usize;
    let (mut in_str, mut esc) = (false, false);
    for (j, ch) in rest.char_indices() {
        if in_str {
            if esc {
                esc = false;
            } else if ch == '\\' {
                esc = true;
            } else if ch == '"' {
                in_str = false;
            }
        } else if ch == '"' {
            in_str = true;
        } else if ch == '{' {
            depth += 1;
        } else if ch == '}' {
            depth -= 1;
            if depth == 0 {
                return Some(&rest[..j]);
            }
        }
    }
    None
}

fn resolve(base: &std::path::Path, p: String) -> std::path::PathBuf {
    let p = std::path::PathBuf::from(p);
    if p.is_absolute() { p } else { base.join(p) }
}

fn read(p: &std::path::Path) -> Result<String, String> {
    std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))
}

struct MeshEntry {
    id: u32,
    obj: String,
    png: String,
}

fn mesh_entries(txt: &str) -> Vec<MeshEntry> {
    let mut out = Vec::new();
    for seg in txt.split("{\"id\": ").skip(1) {
        let seg = &seg[..seg.find('}').unwrap_or(seg.len())];
        out.push(MeshEntry {
            id: seg
                .split(',')
                .next()
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(0),
            obj: field(seg, "obj").unwrap_or_default(),
            png: field(seg, "png").unwrap_or_default(),
        });
    }
    out
}

/// 从对象网格包读入全部对象与布局，组装成 GLB 的 `Part` 列表。
pub struct Loaded {
    pub parts: Vec<Part>,
    pub mesh_manifest: std::path::PathBuf,
    pub objects_manifest: std::path::PathBuf,
    pub layout: std::path::PathBuf,
    pub verts: usize,
    pub tris: usize,
}

pub fn load_scene(
    objects_mesh_dir: &std::path::Path,
    layout_override: Option<&std::path::Path>,
) -> Result<Loaded, String> {
    let mm = objects_mesh_dir.join("objects-mesh.manifest.json");
    let txt = read(&mm)?;
    let inputs = object_block(&txt, "inputs").ok_or("清单缺 inputs")?;
    let oman = object_block(inputs, "objects_manifest").ok_or("清单缺 objects_manifest")?;
    let om_path = resolve(
        objects_mesh_dir,
        field(oman, "path").ok_or("缺对象清单路径")?,
    );
    let lp = match layout_override {
        Some(p) => p.to_path_buf(),
        None => {
            let lb = object_block(inputs, "layout").ok_or("清单没有布局输入")?;
            resolve(objects_mesh_dir, field(lb, "path").ok_or("缺布局路径")?)
        }
    };
    let om = read(&om_path)?;
    let mut bbox: HashMap<u32, ([f64; 3], [f64; 3])> = HashMap::new();
    for seg in om.split("{\"id\": ").skip(1) {
        let seg = &seg[..seg.find('}').unwrap_or(seg.len())];
        let id = seg
            .split(',')
            .next()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        let (lo, hi) = (nums(seg, "bbox_min"), nums(seg, "bbox_max"));
        if lo.len() == 3 && hi.len() == 3 {
            bbox.insert(id, ([lo[0], lo[1], lo[2]], [hi[0], hi[1], hi[2]]));
        }
    }
    let lt = read(&lp)?;
    let mut places: HashMap<u32, (u32, [f64; 3])> = HashMap::new();
    for seg in lt.split("{\"id\": ").skip(1) {
        let seg = &seg[..seg.find('}').unwrap_or(seg.len())];
        let id = seg
            .split(',')
            .next()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        let tr = nums(seg, "translate");
        if tr.len() == 3 {
            places.insert(
                id,
                (
                    number(seg, "yaw").unwrap_or(0.0) as u32,
                    [tr[0], tr[1], tr[2]],
                ),
            );
        }
    }
    let entries = mesh_entries(&txt);
    let mut parts = Vec::new();
    let (mut verts, mut tris) = (0usize, 0usize);
    for e in entries {
        let objtxt = read(&objects_mesh_dir.join(&e.obj))?;
        let (mesh, uv) = parse_obj(&objtxt)?;
        let png =
            std::fs::read(objects_mesh_dir.join(&e.png)).map_err(|x| format!("{}: {x}", e.png))?;
        let (bmin, bmax) = *bbox.get(&e.id).ok_or(format!("对象 {} 缺 bbox", e.id))?;
        let (yaw, t) = *places.get(&e.id).ok_or(format!("对象 {} 缺布局", e.id))?;
        let (translation, rotation) = node_trs(bmin, bmax, yaw, t);
        verts += mesh.verts.len();
        tris += mesh.tris.len();
        parts.push(Part {
            name: format!("obj-{}", e.id),
            mesh,
            uv,
            png,
            translation,
            rotation,
        });
    }
    if parts.is_empty() {
        return Err("没有可打包的对象".into());
    }
    Ok(Loaded {
        parts,
        mesh_manifest: mm,
        objects_manifest: om_path,
        layout: lp,
        verts,
        tris,
    })
}

/// 解析本仓 `uv::write_obj` 写出的 OBJ（v / vt / f 三样）。
pub fn parse_obj(text: &str) -> Result<(Mesh, UvTris), String> {
    let mut verts = Vec::new();
    let mut uvs = Vec::new();
    let mut tris = Vec::new();
    let mut tri_uv = Vec::new();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        match it.next() {
            Some("v") => {
                let q: Vec<f32> = it.filter_map(|s| s.parse().ok()).collect();
                if q.len() < 3 {
                    return Err("v 少于 3 个分量".into());
                }
                verts.push([q[0], q[1], q[2]]);
            }
            Some("vt") => {
                let q: Vec<f32> = it.filter_map(|s| s.parse().ok()).collect();
                if q.len() < 2 {
                    return Err("vt 少于 2 个分量".into());
                }
                uvs.push([q[0], q[1]]);
            }
            Some("f") => {
                let mut vi = [0u32; 3];
                let mut ti = [0u32; 3];
                for k in 0..3 {
                    let c = it.next().ok_or("f 少于 3 个角")?;
                    let mut parts = c.split('/');
                    vi[k] = parts
                        .next()
                        .and_then(|s| s.parse::<u32>().ok())
                        .ok_or("f 顶点号坏")?
                        - 1;
                    ti[k] = parts
                        .next()
                        .and_then(|s| s.parse::<u32>().ok())
                        .unwrap_or(vi[k] + 1)
                        - 1;
                }
                tris.push(vi);
                tri_uv.push([
                    uvs[ti[0] as usize],
                    uvs[ti[1] as usize],
                    uvs[ti[2] as usize],
                ]);
            }
            _ => {}
        }
    }
    if tris.is_empty() || verts.is_empty() {
        return Err("OBJ 空".into());
    }
    Ok((Mesh { verts, tris }, tri_uv))
}

#[cfg(test)]
mod tests {
    use super::*;

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
}

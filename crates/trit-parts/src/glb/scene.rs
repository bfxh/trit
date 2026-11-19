//! 从对象网格包读入全部对象与布局，组装成 GLB 的 `Part` 列表。
//!
//! 下沉自 `glb/mod.rs`：读外部 JSON 的这半段和写 GLB 那半段不在同一条修改路径上，
//! 拆开以后各留各的预算。

use std::collections::HashMap;

use super::manifest::{field, mesh_entries, number, nums, object_block, read, resolve};
use super::{Part, node_trs, parse_obj};

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
    )?;
    let lp = match layout_override {
        Some(p) => p.to_path_buf(),
        None => {
            let lb = object_block(inputs, "layout").ok_or("清单没有布局输入")?;
            resolve(objects_mesh_dir, field(lb, "path").ok_or("缺布局路径")?)?
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
        let objtxt = read(&resolve(objects_mesh_dir, e.obj)?)?;
        let (mesh, uv) = parse_obj(&objtxt)?;
        let png = std::fs::read(resolve(objects_mesh_dir, e.png)?)
            .map_err(|x| format!("{}: {x}", e.id))?;
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

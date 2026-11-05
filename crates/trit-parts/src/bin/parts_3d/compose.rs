//! `compose-scene`：把对象资产按包围盒货架式摆成一个新场景（N4 第一档）。
//!
//! ```text
//! parts-3d compose-scene --objects <对象目录> --out <目录> [--gap 0.05]
//! ```
//!
//! 输入是 `objects.manifest.json`（`export-objects` 的产物）；输出 `composed.ply`
//! 与 `compose.manifest.json`（每对象 yaw/平移/世界包围盒 + 门：穿插体积 0、覆盖率）。
//! 这是确定性基线；学习的布局策略是 N4 后续。

use std::collections::HashMap;
use std::path::PathBuf;

use trit_parts::splat::{
    Item, Placement, apply, overlap_volume, pack_best, read_ply_3dgs, write_ply_3dgs,
};

use super::cli::{arg, fail, usage};

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

struct Obj {
    id: u32,
    path: String,
    count: usize,
    bbox_min: [f64; 3],
    bbox_max: [f64; 3],
}

fn objects(txt: &str) -> Vec<Obj> {
    let mut out = Vec::new();
    for seg in txt.split("{\"id\": ").skip(1) {
        let seg = &seg[..seg.find('}').unwrap_or(seg.len())];
        let id = seg
            .split(',')
            .next()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        let (lo, hi) = (nums(seg, "bbox_min"), nums(seg, "bbox_max"));
        if lo.len() != 3 || hi.len() != 3 {
            continue;
        }
        out.push(Obj {
            id,
            path: field(seg, "path").unwrap_or_default(),
            count: number(seg, "count").unwrap_or(0.0) as usize,
            bbox_min: [lo[0], lo[1], lo[2]],
            bbox_max: [hi[0], hi[1], hi[2]],
        });
    }
    out
}

fn json_nums(v: &[f64]) -> String {
    v.iter()
        .map(|x| format!("{x}"))
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) fn compose_scene_cmd(argv: &[String]) {
    let objdir = PathBuf::from(arg(argv, "--objects").unwrap_or_else(|| usage()));
    let out = PathBuf::from(arg(argv, "--out").unwrap_or_else(|| usage()));
    let gap: f64 = arg(argv, "--gap")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.05);
    let cov_min: f64 = arg(argv, "--cov-min")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.4);
    let ompath = objdir.join("objects.manifest.json");
    let om = std::fs::read_to_string(&ompath)
        .unwrap_or_else(|e| fail(&format!("{}: {e}", ompath.display())));
    let objs = objects(&om);
    if objs.len() < 2 {
        fail("对象少于 2 个，没有合成的意义");
    }
    let mut splats: HashMap<u32, Vec<trit_parts::splat::Splat>> = HashMap::new();
    for o in &objs {
        let p = objdir.join(&o.path);
        let v = read_ply_3dgs(&p).unwrap_or_else(|e| fail(&e));
        if v.len() != o.count {
            fail(&format!(
                "对象 {} 的清单计数 {} ≠ PLY 实际 {}",
                o.id,
                o.count,
                v.len()
            ));
        }
        splats.insert(o.id, v);
    }
    let items: Vec<Item> = objs
        .iter()
        .map(|o| Item {
            id: o.id,
            bbox_min: o.bbox_min,
            bbox_max: o.bbox_max,
        })
        .collect();
    let (placements, width, cov) = pack_best(&items, gap);
    let mut composed = Vec::new();
    for pl in &placements {
        let it = items
            .iter()
            .find(|i| i.id == pl.id)
            .unwrap_or_else(|| fail("摆放 id 对不上"));
        composed.extend(apply(&splats[&pl.id], it, pl));
    }
    let overlap: f64 = (0..placements.len())
        .map(|i| {
            placements
                .iter()
                .skip(i + 1)
                .map(|b| overlap_volume(&placements[i], b))
                .sum::<f64>()
        })
        .sum();
    let gate_ok = overlap <= 1e-12 && cov >= cov_min;

    std::fs::create_dir_all(&out).unwrap_or_else(|e| fail(&format!("{}: {e}", out.display())));
    let ply = out.join("composed.ply");
    write_ply_3dgs(&ply, &composed).unwrap_or_else(|e| fail(&e));
    let bytes = std::fs::metadata(&ply).map(|m| m.len()).unwrap_or(0);
    let sha = trit_core::fingerprint::file_sha256(&ply).unwrap_or_default();
    println!(
        "摆放 {} 个对象：总宽 {width:.3} m，覆盖率 {cov:.3}，穿插体积 {overlap:.3e} → {}",
        placements.len(),
        ply.display()
    );
    for p in &placements {
        println!(
            "  id {}：yaw {}°，平移 ({:.3}, {:.3}, {:.3})，世界包围盒 {:.2}×{:.2}×{:.2}",
            p.id,
            p.yaw,
            p.translate[0],
            p.translate[1],
            p.translate[2],
            p.bbox_max[0] - p.bbox_min[0],
            p.bbox_max[1] - p.bbox_min[1],
            p.bbox_max[2] - p.bbox_min[2]
        );
    }
    if !gate_ok {
        fail(&format!("门未过：覆盖率 {cov:.3} < {cov_min}"));
    }

    let commit = trit_parts::partsrun::git_out(&["rev-parse", "--short", "HEAD"])
        .trim()
        .to_string();
    let dirty = !trit_parts::partsrun::git_out(&["status", "--porcelain"])
        .trim()
        .is_empty();
    let argv_json: Vec<String> = std::env::args()
        .skip(1)
        .map(|a| trit_parts::partsrun::json_str(&a))
        .collect();
    let ordered = objs_iter(&objs, &placements);
    let layout_json: Vec<String> = placements
        .iter()
        .zip(&ordered)
        .map(|(p, o)| {
            format!(
                "{{\"id\": {}, \"yaw\": {}, \"translate\": [{}], \"bbox_min\": [{}], \"bbox_max\": [{}], \"count\": {}}}",
                p.id,
                p.yaw,
                json_nums(&p.translate),
                json_nums(&p.bbox_min),
                json_nums(&p.bbox_max),
                o.count
            )
        })
        .collect();
    let omsha = trit_core::fingerprint::file_sha256(&ompath).unwrap_or_default();
    let body = format!(
        "{{\n  \"schema\": 1,\n  \"kind\": \"extract-compose\",\n  \"created_unix\": {},\n  \"commit\": {},\n  \"dirty\": {dirty},\n  \"argv\": [{}],\n  \"config\": {{\"gap\": {gap}, \"cov_min\": {cov_min}, \"width\": {width}}},\n  \"inputs\": {{\"objects_manifest\": {{\"path\": {}, \"sha256\": \"{omsha}\"}}}},\n  \"layout\": [{}],\n  \"composed\": {{\"path\": \"composed.ply\", \"sha256\": \"{sha}\", \"bytes\": {bytes}, \"count\": {}}},\n  \"gate\": {{\"overlap\": {overlap}, \"coverage\": {cov}, \"cov_min\": {cov_min}, \"ok\": {gate_ok}}}\n}}\n",
        trit_parts::partsrun::created_unix(),
        trit_parts::partsrun::json_str(&commit),
        argv_json.join(", "),
        trit_parts::partsrun::json_str(&trit_parts::scenefp::norm_root(&ompath)),
        layout_json.join(", "),
        composed.len()
    );
    let mp = out.join("compose.manifest.json");
    std::fs::write(&mp, body).unwrap_or_else(|e| fail(&format!("{}: {e}", mp.display())));
    println!("清单 {}", mp.display());
}

/// 按摆放顺序取对象（`pack_best` 已按面积排序）。
fn objs_iter<'a>(objs: &'a [Obj], ps: &'a [Placement]) -> Vec<&'a Obj> {
    ps.iter()
        .map(|p| objs.iter().find(|o| o.id == p.id).unwrap_or(&objs[0]))
        .collect()
}

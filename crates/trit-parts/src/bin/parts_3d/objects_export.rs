//! `export-objects`：按分割 id 把多视角表面拆成**每对象一份高斯资产**。
//!
//! ```text
//! parts-3d export-objects --scene <dir> --frames 0,...,7 --cell 0.01 --out <dir> [--ids 1,2,5]
//! ```
//!
//! 这是场景组合（N4）的地基：对象各自有文件、包围盒与质心，可单独移动/替换/删除；
//! 清单里记内容级指纹与每对象的 SHA/计数/包围盒，复核器会**重新建一遍**再逐字节比对。

use std::collections::BTreeSet;
use std::path::PathBuf;

use trit_parts::data::{read_npy_i32, read_rgb};
use trit_parts::fuse::View;
use trit_parts::geo::{Camera, read_npy_f32};
use trit_parts::splat::{build_filtered, sort_splats, write_ply_3dgs};

use super::cli::{arg, fail, frame_path, usage};

fn parse_list(argv: &[String], k: &str, d: Vec<usize>) -> Vec<usize> {
    arg(argv, k)
        .map(|v| v.split(',').filter_map(|s| s.trim().parse().ok()).collect())
        .unwrap_or(d)
}

fn load_view(scene: &std::path::Path, f: usize) -> View {
    let (ids, shape) =
        read_npy_i32(&frame_path(scene, "seg", f, "npy")).unwrap_or_else(|e| fail(&e));
    let (d, _) = read_npy_f32(&frame_path(scene, "depth", f, "npy")).unwrap_or_else(|e| fail(&e));
    View {
        ids: ids.iter().map(|&v| v.max(0) as u32).collect(),
        depth: d,
        w: shape[1],
        h: shape[0],
        cam: Camera::load(&frame_path(scene, "meta", f, "json")).unwrap_or_else(|e| fail(&e)),
    }
}

fn load_rgb(scene: &std::path::Path, f: usize) -> Vec<f32> {
    read_rgb(&frame_path(scene, "rgb", f, "png"))
        .map(|(_, _, p)| p)
        .unwrap_or_else(|e| fail(&e))
}

pub(crate) fn objects_export_cmd(argv: &[String]) {
    let scene = PathBuf::from(arg(argv, "--scene").unwrap_or_else(|| usage()));
    let out = PathBuf::from(arg(argv, "--out").unwrap_or_else(|| usage()));
    let frames = parse_list(argv, "--frames", vec![0, 1, 2, 3, 4, 5, 6, 7]);
    let scenes = arg(argv, "--scenes").map(PathBuf::from);
    let labels = scenes.as_ref().map(|root| {
        let name = scene
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        let p = root.join(name).join("objects.json");
        trit_parts::objlabel::load(&p).unwrap_or_else(|e| fail(&e))
    });
    let cell: f32 = arg(argv, "--cell")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.01);
    let scale_k: f32 = arg(argv, "--scale-k")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.6);
    let alpha: f32 = arg(argv, "--alpha")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.8);
    if frames.is_empty() {
        fail("--frames 不能为空");
    }
    std::fs::create_dir_all(&out).unwrap_or_else(|e| fail(&format!("{}: {e}", out.display())));
    let out = out
        .canonicalize()
        .unwrap_or_else(|e| fail(&format!("{}: {e}", out.display())));

    let views: Vec<View> = frames.iter().map(|&f| load_view(&scene, f)).collect();
    let rgbs: Vec<Vec<f32>> = frames.iter().map(|&f| load_rgb(&scene, f)).collect();
    let mut ids: BTreeSet<u32> = BTreeSet::new();
    for v in &views {
        for &id in &v.ids {
            if id != 0 {
                ids.insert(id);
            }
        }
    }
    if let Some(want) = arg(argv, "--ids") {
        let keep: BTreeSet<u32> = want
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect();
        ids.retain(|id| keep.contains(id));
    }

    let mut entries = Vec::new();
    let mut skipped = Vec::new();
    for &id in &ids {
        let mut splats = build_filtered(&views, &rgbs, cell, scale_k, |v| v == id);
        for s in splats.iter_mut() {
            s.alpha = alpha.clamp(1e-4, 1.0 - 1e-4);
        }
        sort_splats(&mut splats);
        if splats.is_empty() {
            skipped.push(id);
            continue;
        }
        let name = format!("obj-{id}.ply");
        let p = out.join(&name);
        write_ply_3dgs(&p, &splats).unwrap_or_else(|e| fail(&e));
        let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
        let mut cen = [0f64; 3];
        for s in &splats {
            for k in 0..3 {
                lo[k] = lo[k].min(s.pos[k] as f64);
                hi[k] = hi[k].max(s.pos[k] as f64);
                cen[k] += s.pos[k] as f64;
            }
        }
        for c in &mut cen {
            *c /= splats.len() as f64;
        }
        let extra = if let Some(ls) = &labels {
            let l = ls
                .iter()
                .find(|l| l.pass_index == id)
                .unwrap_or_else(|| fail(&format!("对象 {id} 在 objects.json 里没有标签")));
            let err = trit_parts::objlabel::centroid_error(cen, l.center);
            let gd = trit_parts::objlabel::gt_diag(l);
            let our_diag =
                ((hi[0] - lo[0]).powi(2) + (hi[1] - lo[1]).powi(2) + (hi[2] - lo[2]).powi(2))
                    .sqrt();
            let ratio = our_diag / gd.max(1e-9);
            println!(
                "  标签 {}，质心误差 {err:.4} m，对角线比 {ratio:.3}",
                l.kind
            );
            format!(
                ", \"kind\": {}, \"gt_center\": [{}, {}, {}], \"centroid_error\": {err}, \"gt_diag\": {gd}, \"diag_ratio\": {ratio}",
                trit_parts::partsrun::json_str(&l.kind),
                l.center[0],
                l.center[1],
                l.center[2]
            )
        } else {
            String::new()
        };
        let bytes = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
        let sha = trit_core::fingerprint::file_sha256(&p).unwrap_or_default();
        println!(
            "对象 {id}: {} 个高斯，包围盒 {:.3}×{:.3}×{:.3} m，{} B → {name}",
            splats.len(),
            hi[0] - lo[0],
            hi[1] - lo[1],
            hi[2] - lo[2],
            bytes
        );
        entries.push(format!(
            "{{\"id\": {id}, \"path\": {}, \"sha256\": \"{sha}\", \"bytes\": {bytes}, \"count\": {}, \"bbox_min\": [{}, {}, {}], \"bbox_max\": [{}, {}, {}], \"centroid\": [{}, {}, {}]{extra}}}",
            trit_parts::partsrun::json_str(&name),
            splats.len(),
            lo[0],
            lo[1],
            lo[2],
            hi[0],
            hi[1],
            hi[2],
            cen[0],
            cen[1],
            cen[2]
        ));
    }
    if entries.is_empty() {
        fail("没有任何对象有可见像素");
    }

    let fp = trit_parts::scenefp::fingerprint_scene_frames_rgb(&scene, &frames)
        .unwrap_or_else(|e| fail(&e));
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
    let frames_json: Vec<String> = frames.iter().map(|f| f.to_string()).collect();
    let scenes_json = scenes
        .as_ref()
        .map(|p| trit_parts::partsrun::json_str(&trit_parts::scenefp::norm_root(p)))
        .unwrap_or_else(|| "null".into());
    let skipped_json: Vec<String> = skipped.iter().map(|i| i.to_string()).collect();
    let body = format!(
        "{{\n  \"schema\": 1,\n  \"kind\": \"extract-objects\",\n  \"created_unix\": {},\n  \"commit\": {},\n  \"dirty\": {dirty},\n  \"argv\": [{}],\n  \"config\": {{\"frames\": [{}], \"cell\": {cell}, \"scale_k\": {scale_k}, \"alpha\": {alpha}, \"scenes\": {scenes_json}}},\n  \"data\": {{\"files\": {}, \"bytes\": {}, \"algo\": \"sha256-content-v1\", \"hash\": \"sha256:{}\", \"scene\": {}}},\n  \"objects\": [{}],\n  \"skipped\": [{}]\n}}\n",
        trit_parts::partsrun::created_unix(),
        trit_parts::partsrun::json_str(&commit),
        argv_json.join(", "),
        frames_json.join(", "),
        fp.files,
        fp.bytes,
        fp.hash,
        trit_parts::partsrun::json_str(&trit_parts::scenefp::norm_root(&scene)),
        entries.join(", "),
        skipped_json.join(", ")
    );
    let mp = out.join("objects.manifest.json");
    std::fs::write(&mp, body).unwrap_or_else(|e| fail(&format!("{}: {e}", mp.display())));
    println!(
        "对象资产 {} 个（跳过 {} 个空对象）→ {}",
        entries.len(),
        skipped.len(),
        mp.display()
    );
}

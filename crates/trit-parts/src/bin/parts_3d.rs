//! `parts-3d`：部件 id 图 + 深度 → 3D 点云（PLY），以及逐部件 3D IoU 评估。
//!
//! 四个子命令：
//! - `cloud`：GT seg + depth → 逐部件着色的 `.ply`；
//! - `infer`：模型预测 id + **外部深度**（任意来源，默认取 scene/depth）→ `.ply`（生产路径）；
//! - `eval`：模型预测 id + GT depth → 与 GT 反投影逐部件比对（3D IoU / 质心误差）；
//! - `mesh`：点云 → 实心占用 → Surface Nets 网格，并按 `--levels` 出 LOD 金字塔。

use std::path::{Path, PathBuf};

use trit_parts::data::read_npy_i32;
use trit_parts::diffuse::{Denoiser, PatchMlp, Schedule};
use trit_parts::geo::{Camera, centroid, clouds, iou3d, read_npy_f32, voxelize, write_ply};
use trit_parts::tconv::Net;

fn arg(argv: &[String], key: &str) -> Option<String> {
    let i = argv.iter().position(|a| a == key)?;
    argv.get(i + 1).cloned()
}

fn frame_path(dir: &Path, sub: &str, n: usize, ext: &str) -> PathBuf {
    dir.join(sub).join(format!("{n:06}.{ext}"))
}

fn cloud(argv: &[String]) {
    let seg = PathBuf::from(arg(argv, "--seg").unwrap_or_else(|| usage()));
    let depth = PathBuf::from(arg(argv, "--depth").unwrap_or_else(|| usage()));
    let meta = PathBuf::from(arg(argv, "--meta").unwrap_or_else(|| usage()));
    let out = PathBuf::from(arg(argv, "--out").unwrap_or_else(|| usage()));

    let cam = Camera::load(&meta).unwrap_or_else(|e| fail(&e));
    let (ids, shape) = read_npy_i32(&seg).unwrap_or_else(|e| fail(&e));
    let (dep, dshape) = read_npy_f32(&depth).unwrap_or_else(|e| fail(&e));
    if shape != dshape || shape.len() != 2 {
        fail("seg 与 depth 形状不一致");
    }
    let (h, w) = (shape[0], shape[1]);
    let ids: Vec<u32> = ids.iter().map(|&v| v.max(0) as u32).collect();
    let parts = clouds(&ids, &dep, &cam, w, h);
    write_ply(&out, &parts).unwrap_or_else(|e| fail(&format!("写 PLY 失败: {e}")));
    let n: usize = parts.values().map(Vec::len).sum();
    println!("部件 {} 个，点 {} 个 → {}", parts.len(), n, out.display());
}

/// `infer --model <tpm> --scene <dir> --frame N --manifest <json> --out <ply> [--depth <npy>]`
/// 模型出部件 id，深度由外部提供（本仓不预测深度），反投影后写逐部件 PLY。
#[cfg(feature = "gpu")]
fn infer(argv: &[String]) {
    use trit_parts::api::{infer_ids, wgpu_from_tpm};
    use trit_parts::data::read_rgb;
    use trit_parts::parts::PartList;
    use trit_parts::tpm::TpmModel;

    let model = PathBuf::from(arg(argv, "--model").unwrap_or_else(|| usage()));
    let scene = PathBuf::from(arg(argv, "--scene").unwrap_or_else(|| usage()));
    let manifest = PathBuf::from(arg(argv, "--manifest").unwrap_or_else(|| usage()));
    let out = PathBuf::from(arg(argv, "--out").unwrap_or_else(|| usage()));
    let frame: usize = arg(argv, "--frame")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let depth_path = arg(argv, "--depth").map(PathBuf::from);

    let tpm = TpmModel::load(&model).unwrap_or_else(|e| fail(&e));
    let parts = PartList::load(&manifest).unwrap_or_else(|e| fail(&e));
    let mut backend = wgpu_from_tpm(&tpm).unwrap_or_else(|e| panic!("wgpu 后端不可用: {e}"));

    let rgb_p = frame_path(&scene, "rgb", frame, "png");
    let (w, _h, rgb) = read_rgb(&rgb_p).unwrap_or_else(|e| fail(&e));
    let ids = match infer_ids(&mut backend, &rgb, w as u32, &parts) {
        Ok(v) => v,
        Err(e) => fail(&format!("推理失败: {e}")),
    };
    let depth = depth_path.unwrap_or_else(|| frame_path(&scene, "depth", frame, "npy"));
    let (dep, shape) = read_npy_f32(&depth).unwrap_or_else(|e| fail(&e));
    if shape.len() != 2 || shape[0] * shape[1] != ids.len() {
        fail("深度与 id 图尺寸不一致");
    }
    let cam = Camera::load(&frame_path(&scene, "meta", frame, "json")).unwrap_or_else(|e| fail(&e));
    let parts3 = clouds(&ids, &dep, &cam, shape[1], shape[0]);
    write_ply(&out, &parts3).unwrap_or_else(|e| fail(&format!("写 PLY 失败: {e}")));
    let n: usize = parts3.values().map(Vec::len).sum();
    println!(
        "帧 {frame}：部件 {} 个，点 {} 个 → {}",
        parts3.len(),
        n,
        out.display()
    );
}

/// `infer-ids --model <tpm> --scene <dir> --manifest <parts.json> --frames 0,1,… [--out <dir>]`：
/// 逐帧跑部件头，**输出预测 id 图**（npy）并与 seg 对拍（只在前景像素上算）。
/// 供时序 id 抖动量测用：`python scripts/id_jitter.py <pred_dir> <scene>`。
#[cfg(feature = "gpu")]
fn infer_ids_cmd(argv: &[String]) {
    use trit_parts::api::{infer_ids, wgpu_from_tpm};
    use trit_parts::data::{read_npy_i32, read_rgb, write_npy_i32};
    use trit_parts::parts::PartList;
    use trit_parts::tpm::TpmModel;

    let model = PathBuf::from(arg(argv, "--model").unwrap_or_else(|| usage()));
    let scene = PathBuf::from(arg(argv, "--scene").unwrap_or_else(|| usage()));
    let manifest = PathBuf::from(arg(argv, "--manifest").unwrap_or_else(|| usage()));
    let frames: Vec<usize> = arg(argv, "--frames")
        .map(|v| v.split(',').filter_map(|s| s.trim().parse().ok()).collect())
        .unwrap_or_else(|| vec![0]);
    let out_dir = arg(argv, "--out").map(PathBuf::from);

    let tpm = TpmModel::load(&model).unwrap_or_else(|e| fail(&e));
    let parts = PartList::load(&manifest).unwrap_or_else(|e| fail(&e));
    let mut backend = wgpu_from_tpm(&tpm).unwrap_or_else(|e| panic!("wgpu 后端不可用: {e}"));
    if let Some(d) = &out_dir {
        std::fs::create_dir_all(d).unwrap_or_else(|e| fail(&format!("{}: {e}", d.display())));
    }
    for &fr in &frames {
        let (w, h, rgb) =
            read_rgb(&frame_path(&scene, "rgb", fr, "png")).unwrap_or_else(|e| fail(&e));
        let ids = match infer_ids(&mut backend, &rgb, w as u32, &parts) {
            Ok(v) => v,
            Err(e) => fail(&format!("推理失败: {e}")),
        };
        let (gt, shape) =
            read_npy_i32(&frame_path(&scene, "seg", fr, "npy")).unwrap_or_else(|e| fail(&e));
        if shape.len() != 2 || shape[0] != h || shape[1] != w {
            fail("seg 与 rgb 尺寸不一致");
        }
        let mut ok = 0usize;
        let mut fg = 0usize;
        for i in 0..w * h {
            if gt[i] > 0 {
                fg += 1;
                if ids[i] == gt[i] as u32 {
                    ok += 1;
                }
            }
        }
        println!(
            "帧 {fr}：前景 {fg} 像素，逐像素一致 {ok} = {:.3}",
            ok as f32 / fg.max(1) as f32
        );
        if let Some(d) = &out_dir {
            let v: Vec<i32> = ids.iter().map(|&x| x as i32).collect();
            write_npy_i32(&d.join(format!("{fr:06}.npy")), &v, w, h).unwrap_or_else(|e| fail(&e));
        }
    }
}

#[cfg(feature = "gpu")]
fn eval(argv: &[String]) {
    use trit_parts::api::{infer_ids, wgpu_from_tpm};
    use trit_parts::data::read_rgb;
    use trit_parts::parts::PartList;
    use trit_parts::tpm::TpmModel;

    let model = PathBuf::from(arg(argv, "--model").unwrap_or_else(|| usage()));
    let scene = PathBuf::from(arg(argv, "--scene").unwrap_or_else(|| usage()));
    let manifest = PathBuf::from(arg(argv, "--manifest").unwrap_or_else(|| usage()));
    let limit: usize = arg(argv, "--limit")
        .and_then(|v| v.parse().ok())
        .unwrap_or(usize::MAX);
    let voxel: f32 = arg(argv, "--voxel")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.01);
    let min_vox: usize = arg(argv, "--min-vox")
        .and_then(|v| v.parse().ok())
        .unwrap_or(30);
    // 自检：用 GT 当预测（度量应给 1.0）。度量本身不准时这里会先红。
    let gt_as_pred = argv.iter().any(|a| a == "--gt");
    let fuse: usize = arg(argv, "--fuse")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let min_votes: u32 = arg(argv, "--min-votes")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let mut gt_acc: std::collections::HashMap<
        u32,
        std::collections::HashSet<trit_parts::geo::Voxel>,
    > = std::collections::HashMap::new();
    // 预测侧按体素投票（union 会把逐帧噪声越积越多，实测 IoU 反而下降）。
    let mut votes: std::collections::HashMap<
        trit_parts::geo::Voxel,
        std::collections::HashMap<u32, u32>,
    > = std::collections::HashMap::new();

    let tpm = TpmModel::load(&model).unwrap_or_else(|e| fail(&e));
    let parts = PartList::load(&manifest).unwrap_or_else(|e| fail(&e));
    let mut backend = wgpu_from_tpm(&tpm).unwrap_or_else(|e| panic!("wgpu 后端不可用: {e}"));

    let mut ious: Vec<f32> = Vec::new();
    let mut cdist: Vec<f32> = Vec::new();
    let mut frames = 0usize;
    for n in 0..limit {
        let rgb_p = frame_path(&scene, "rgb", n, "png");
        if !rgb_p.exists() {
            break;
        }
        let (w, _h, rgb) = read_rgb(&rgb_p).unwrap_or_else(|e| fail(&e));
        let (gt, shape) =
            read_npy_i32(&frame_path(&scene, "seg", n, "npy")).unwrap_or_else(|e| fail(&e));
        let (dep, ds) =
            read_npy_f32(&frame_path(&scene, "depth", n, "npy")).unwrap_or_else(|e| fail(&e));
        if shape != ds {
            fail("seg 与 depth 形状不一致");
        }
        let (h, sw) = (shape[0], shape[1]);
        let cam = Camera::load(&frame_path(&scene, "meta", n, "json")).unwrap_or_else(|e| fail(&e));
        let gt_ids: Vec<u32> = gt.iter().map(|&v| v.max(0) as u32).collect();
        let pred = if gt_as_pred {
            gt_ids.clone()
        } else {
            match infer_ids(&mut backend, &rgb, w as u32, &parts) {
                Ok(v) => v,
                Err(e) => fail(&format!("推理失败: {e}")),
            }
        };
        if std::env::var("DEBUG_3D").is_ok() {
            let np = pred.len();
            let k = pred.iter().filter(|&&v| v > 0).count();
            let hit = (0..np)
                .filter(|&i| pred[i] > 0 && pred[i] == gt_ids[i])
                .count();
            println!(
                "dbg frame {n}: pred px {} ({:.3}), id-hit {}",
                k,
                k as f64 / np as f64,
                hit
            );
        }
        let gt_parts = clouds(&gt_ids, &dep, &cam, sw, h);
        let pred_parts = clouds(&pred, &dep, &cam, sw, h);
        if fuse > 1 {
            for (id, ppts) in &pred_parts {
                for v in voxelize(ppts, voxel) {
                    *votes.entry(v).or_default().entry(*id).or_insert(0) += 1;
                }
            }
        }

        for (id, gpts) in &gt_parts {
            let gvox = voxelize(gpts, voxel);
            if fuse > 1 {
                gt_acc.entry(*id).or_default().extend(gvox);
                continue;
            }
            if gvox.len() < min_vox {
                continue;
            }
            let pvox = pred_parts
                .get(id)
                .map(|p| voxelize(p, voxel))
                .unwrap_or_default();
            let i = iou3d(&gvox, &pvox);
            if i.is_finite() {
                ious.push(i);
            }
            if let Some(ppts) = pred_parts.get(id) {
                let (a, b) = (centroid(gpts), centroid(ppts));
                cdist.push(
                    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt(),
                );
            }
        }
        frames += 1;
    }

    if fuse > 1 {
        let mut pred_acc: std::collections::HashMap<
            u32,
            std::collections::HashSet<trit_parts::geo::Voxel>,
        > = std::collections::HashMap::new();
        for (v, cnt) in &votes {
            if let Some((&id, c)) = cnt.iter().max_by_key(|(_, c)| **c)
                && *c >= min_votes
            {
                pred_acc.entry(id).or_default().insert(*v);
            }
        }
        for (id, gvox) in &gt_acc {
            if gvox.len() < min_vox {
                continue;
            }
            let empty = std::collections::HashSet::new();
            let pvox = pred_acc.get(id).unwrap_or(&empty);
            let i = iou3d(gvox, pvox);
            if i.is_finite() {
                ious.push(i);
            }
        }
    }
    let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len().max(1) as f32;
    let mut sorted = ious.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let med = if sorted.is_empty() {
        f32::NAN
    } else {
        sorted[sorted.len() / 2]
    };
    println!(
        "帧 {frames}，部件样本 {}，体素 {:.0} mm，GT≥{min_vox} 体素",
        ious.len(),
        voxel * 1000.0
    );
    println!(
        "逐部件 3D IoU 均值 {:.4} / 中位 {:.4}；质心误差 均值 {:.1} mm",
        mean(&ious),
        med,
        mean(&cdist) * 1000.0
    );
    if gt_as_pred {
        let ok = (mean(&ious) - 1.0).abs() < 1e-4 && mean(&cdist) < 1e-6;
        println!("自检 --gt：{}", if ok { "通过（IoU=1.0）" } else { "失败" });
        if !ok {
            fail("自检失败：GT 对 GT 的 3D 度量不是恒等，度量有 bug");
        }
    }
}

/// `mesh --seg <npy> --depth <npy> --meta <json> --out <ply> [--cell 0.01] [--levels 0,2,4]`：
/// 单帧点云 → 实心占用 → Surface Nets；`--levels` 是降采样倍数序列（0=原始精度），
/// 每个都在 `<out>` 旁写 `<stem>.lod<K>.ply`，并打印顶点/三角面/体积/非流形边数。
fn mesh_cmd(argv: &[String]) {
    use trit_parts::mesh::{
        Grid, downsample_occupancy, fill_solid, non_manifold_edges, surface_nets,
        write_ply as write_mesh,
    };

    let seg = PathBuf::from(arg(argv, "--seg").unwrap_or_else(|| usage()));
    let depth = PathBuf::from(arg(argv, "--depth").unwrap_or_else(|| usage()));
    let meta = PathBuf::from(arg(argv, "--meta").unwrap_or_else(|| usage()));
    let out = PathBuf::from(arg(argv, "--out").unwrap_or_else(|| usage()));
    let cell: f32 = arg(argv, "--cell")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.01);
    let levels: Vec<usize> = arg(argv, "--levels")
        .map(|v| v.split(',').filter_map(|s| s.trim().parse().ok()).collect())
        .unwrap_or_else(|| vec![0]);

    let cam = Camera::load(&meta).unwrap_or_else(|e| fail(&e));
    let (ids, shape) = read_npy_i32(&seg).unwrap_or_else(|e| fail(&e));
    let (dep, dshape) = read_npy_f32(&depth).unwrap_or_else(|e| fail(&e));
    if shape != dshape || shape.len() != 2 {
        fail("seg 与 depth 形状不一致");
    }
    let (h, w) = (shape[0], shape[1]);
    let ids: Vec<u32> = ids.iter().map(|&v| v.max(0) as u32).collect();
    let parts = clouds(&ids, &dep, &cam, w, h);
    let pts: Vec<[f32; 3]> = parts
        .values()
        .flat_map(|v| v.iter().map(|p| [p[0], p[1], p[2]]))
        .collect();
    let mut g = Grid::from_points(&pts, cell, 2).unwrap_or_else(|| fail("点云为空"));
    fill_solid(&mut g);
    for f in levels {
        let gg = if f <= 1 {
            g.clone()
        } else {
            downsample_occupancy(&g, f)
        };
        let m = surface_nets(&gg);
        let name = if f <= 1 {
            out.clone()
        } else {
            let stem = out.file_stem().and_then(|s| s.to_str()).unwrap_or("mesh");
            out.with_file_name(format!("{stem}.lod{f}.ply"))
        };
        write_mesh(&name, &m).unwrap_or_else(|e| fail(&e));
        println!(
            "lod{f}: 栅格 {}×{}×{} cell {:.4} m → 顶点 {} 三角面 {} 体积 {:.4} m³ 非流形边 {} → {}",
            gg.nx,
            gg.ny,
            gg.nz,
            gg.cell,
            m.verts.len(),
            m.tris.len(),
            trit_parts::mesh::mesh_volume(&m),
            non_manifold_edges(&m),
            name.display()
        );
    }
}

/// `fuse --scene <dir> --frames 0,1,2,3 [--eval 4,5,6,7] --cell 0.01 --out <ply>`：
/// 多视角体素融合 → 场景网格 + 逐部件网格；给了 `--eval` 就用这些视角的 GT 点云
/// 算逐部件 3D IoU（重建只用 `--frames`，评测视角不参与）。
fn fuse_cmd(argv: &[String]) {
    use trit_parts::fuse::{FuseCfg, View, fuse};
    use trit_parts::geo::clouds;
    use trit_parts::mesh::{
        downsample_occupancy, non_manifold_edges, surface_nets, write_ply as write_mesh,
    };

    let scene = PathBuf::from(arg(argv, "--scene").unwrap_or_else(|| usage()));
    let out = PathBuf::from(arg(argv, "--out").unwrap_or_else(|| usage()));
    let parse_list = |k: &str, d: Vec<usize>| -> Vec<usize> {
        arg(argv, k)
            .map(|v| v.split(',').filter_map(|s| s.trim().parse().ok()).collect())
            .unwrap_or(d)
    };
    let frames = parse_list("--frames", vec![0, 1, 2, 3]);
    let eval_frames = parse_list("--eval", Vec::new());
    let cell: f32 = arg(argv, "--cell")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.01);
    let min_votes: f32 = arg(argv, "--min-votes")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1.0);
    let seal: usize = arg(argv, "--seal")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let voxel: f32 = arg(argv, "--voxel")
        .and_then(|v| v.parse().ok())
        .unwrap_or(cell);
    let levels = parse_list("--levels", vec![0]);

    let load = |f: usize| -> View {
        let seg = frame_path(&scene, "seg", f, "npy");
        let dep = frame_path(&scene, "depth", f, "npy");
        let meta = frame_path(&scene, "meta", f, "json");
        let (ids, shape) = read_npy_i32(&seg).unwrap_or_else(|e| fail(&e));
        let (d, dshape) = read_npy_f32(&dep).unwrap_or_else(|e| fail(&e));
        if shape != dshape || shape.len() != 2 {
            fail("seg 与 depth 形状不一致");
        }
        View {
            ids: ids.iter().map(|&v| v.max(0) as u32).collect(),
            depth: d,
            w: shape[1],
            h: shape[0],
            cam: Camera::load(&meta).unwrap_or_else(|e| fail(&e)),
        }
    };
    let views: Vec<View> = frames.iter().map(|&f| load(f)).collect();
    let fused = fuse(
        &views,
        &FuseCfg {
            cell,
            margin: 2,
            min_votes,
            seal,
        },
    )
    .unwrap_or_else(|| fail("融合失败：没有有效深度"));

    // 场景网格 + LOD
    let sg = fused.scene_grid();
    for f in &levels {
        let gg = if *f <= 1 {
            sg.clone()
        } else {
            downsample_occupancy(&sg, *f)
        };
        let m = surface_nets(&gg);
        let name = if *f <= 1 {
            out.clone()
        } else {
            let stem = out.file_stem().and_then(|s| s.to_str()).unwrap_or("scene");
            out.with_file_name(format!("{stem}.lod{f}.ply"))
        };
        write_mesh(&name, &m).unwrap_or_else(|e| fail(&e));
        println!(
            "scene lod{f}: cell {:.3} m 顶点 {} 面 {} 非流形边 {} → {}",
            gg.cell,
            m.verts.len(),
            m.tris.len(),
            non_manifold_edges(&m),
            name.display()
        );
    }

    // 逐部件网格 + 闭合率
    let parts = fused.hull_meshes(&views);
    let closed = parts.iter().filter(|p| p.closed).count();
    let stem = out.file_stem().and_then(|s| s.to_str()).unwrap_or("scene");
    for p in &parts {
        let f = out.with_file_name(format!("{stem}.part{:03}.ply", p.id));
        write_mesh(&f, &p.mesh).unwrap_or_else(|e| fail(&e));
        if !p.closed {
            println!("  部件 {}: 面 {} **不闭合**", p.id, p.mesh.tris.len());
        }
    }
    println!(
        "部件网格 {} 个，闭合 {} 个（{:.1}%）",
        parts.len(),
        closed,
        100.0 * closed as f64 / parts.len().max(1) as f64
    );

    // 逐部件 3D IoU（留出视角 GT 点云 vs 融合出的壳体素）
    if !eval_frames.is_empty() {
        let mut gt: std::collections::BTreeMap<u32, Vec<[f32; 3]>> =
            std::collections::BTreeMap::new();
        for f in &eval_frames {
            let v = load(*f);
            for (id, pts) in clouds(&v.ids, &v.depth, &v.cam, v.w, v.h) {
                gt.entry(id).or_default().extend(pts);
            }
        }
        let _ = voxel;
        let mut ious: Vec<(u32, f32)> = Vec::new();
        for (&id, pts) in &gt {
            // GT 点投到融合栅格（同一格子索引空间）
            let gset: std::collections::HashSet<(i32, i32, i32)> =
                pts.iter().filter_map(|p| fused.cell_index(*p)).collect();
            // 几何口径：**投票壳体素**（这才是多视角融合出来的表面）
            let rset = fused.part_cells(id);
            let hull = parts.iter().find(|p| p.id == id);
            let inter = gset.intersection(&rset).count() as f32;
            let union = (gset.len() + rset.len()) as f32 - inter;
            let iou = if union > 0.0 { inter / union } else { f32::NAN };

            // 体积 IoU：GT 壳填实的实体 vs 外壳体积
            let (nx, ny) = (fused.grid.nx, fused.grid.ny);
            let mut gt_solid = fused.grid.clone();
            for v in gt_solid.occ.iter_mut() {
                *v = 0;
            }
            for &(x, y, z) in &gset {
                gt_solid.occ[x as usize + nx * (y as usize + ny * z as usize)] = 1;
            }
            trit_parts::mesh::fill_solid(&mut gt_solid);
            let gvol: std::collections::HashSet<(i32, i32, i32)> = gt_solid
                .occ
                .iter()
                .enumerate()
                .filter(|(_, v)| **v != 0)
                .map(|(i, _)| {
                    (
                        (i % nx) as i32,
                        ((i / nx) % ny) as i32,
                        (i / (nx * ny)) as i32,
                    )
                })
                .collect();
            let rvol = hull.map(|p| p.volume.clone()).unwrap_or_default();
            let vi = gvol.intersection(&rvol).count() as f32;
            let vu = (gvol.len() + rvol.len()) as f32 - vi;
            let viou = if vu > 0.0 { vi / vu } else { f32::NAN };
            ious.push((id, iou));
            println!(
                "  part {id}: 壳 IoU {iou:.3}（GT {} / 重建 {} 格） 外壳体积 IoU {viou:.3}",
                gset.len(),
                rset.len()
            );
        }
        let vals: Vec<f32> = ious
            .iter()
            .map(|(_, v)| *v)
            .filter(|v| v.is_finite())
            .collect();
        let mean = vals.iter().sum::<f32>() / vals.len().max(1) as f32;
        let min = vals.iter().cloned().fold(f32::INFINITY, f32::min);
        println!(
            "逐部件 3D IoU：均值 {:.3}（{} 个部件）最差 {:.3}；门：均值 ≥0.5 / 闭合率 ≥80%",
            mean,
            vals.len(),
            min
        );
    }
}

/// `splat --scene <dir> --frames 0,2,4,6 --eval 1,3,5,7 --cells 0.04,0.02,0.01`：
/// 精度阶梯：每个采样格同时给出「体素/网格/高斯」三档的数量与留出视角 PSNR。
fn splat_cmd(argv: &[String]) {
    use trit_parts::data::read_rgb;
    use trit_parts::fuse::{FuseCfg, View, fuse};
    use trit_parts::mesh::{non_manifold_edges, surface_nets};
    use trit_parts::splat::{build, render};

    let scene = PathBuf::from(arg(argv, "--scene").unwrap_or_else(|| usage()));
    let parse_list = |k: &str, d: Vec<usize>| -> Vec<usize> {
        arg(argv, k)
            .map(|v| v.split(',').filter_map(|s| s.trim().parse().ok()).collect())
            .unwrap_or(d)
    };
    let mut frames = parse_list("--frames", vec![0, 1, 2, 3, 4, 5, 6, 7]);
    let n_views: usize = arg(argv, "--views")
        .and_then(|v| v.parse().ok())
        .unwrap_or(frames.len());
    frames.truncate(n_views.max(1));
    let eval_frames = parse_list("--eval", vec![1, 3, 5, 7]);
    let cells: Vec<f32> = arg(argv, "--cells")
        .map(|v| v.split(',').filter_map(|s| s.trim().parse().ok()).collect())
        .unwrap_or_else(|| vec![0.04, 0.02, 0.01]);
    let scale_k: f32 = arg(argv, "--scale-k")
        .and_then(|v| v.parse().ok())
        .unwrap_or(2.0);
    let alpha: f32 = arg(argv, "--alpha")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.6);
    let fit: usize = arg(argv, "--fit").and_then(|v| v.parse().ok()).unwrap_or(0);
    let fit_lr: f32 = arg(argv, "--fit-lr")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.5);
    let fit_all_mode = argv.iter().any(|a| a == "--fit-all");
    let lr_pos: f32 = arg(argv, "--fit-pos")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.02);
    let lr_scale: f32 = arg(argv, "--fit-scale")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1e-4);
    let lr_alpha: f32 = arg(argv, "--fit-alpha")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.05);
    let fit_reg: f32 = arg(argv, "--fit-reg")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.0);
    let densify_rounds: usize = arg(argv, "--densify")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let densify_err: f32 = arg(argv, "--densify-err")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.15);

    let load = |f: usize| -> View {
        let (ids, shape) =
            read_npy_i32(&frame_path(&scene, "seg", f, "npy")).unwrap_or_else(|e| fail(&e));
        let (d, _) =
            read_npy_f32(&frame_path(&scene, "depth", f, "npy")).unwrap_or_else(|e| fail(&e));
        View {
            ids: ids.iter().map(|&v| v.max(0) as u32).collect(),
            depth: d,
            w: shape[1],
            h: shape[0],
            cam: Camera::load(&frame_path(&scene, "meta", f, "json")).unwrap_or_else(|e| fail(&e)),
        }
    };
    let fit_views: Vec<View> = frames.iter().map(|&f| load(f)).collect();
    let fit_rgbs: Vec<Vec<f32>> = frames
        .iter()
        .map(|&f| {
            read_rgb(&frame_path(&scene, "rgb", f, "png"))
                .map(|(_, _, p)| p)
                .unwrap_or_else(|e| fail(&e))
        })
        .collect();
    let eval_views: Vec<View> = eval_frames.iter().map(|&f| load(f)).collect();
    let eval_rgbs: Vec<Vec<f32>> = eval_frames
        .iter()
        .map(|&f| {
            read_rgb(&frame_path(&scene, "rgb", f, "png"))
                .map(|(_, _, p)| p)
                .unwrap_or_else(|e| fail(&e))
        })
        .collect();

    println!("cell(m)  体素      网格顶点/面        外壳非流形边   高斯数  PSNR(fg) 拟合前→后");
    for &cell in &cells {
        // 体素/网格档
        let fused = fuse(
            &fit_views,
            &FuseCfg {
                cell,
                margin: 2,
                min_votes: 1.0,
                seal: 1,
            },
        )
        .unwrap_or_else(|| fail("融合失败"));
        let vox = fused.grid.occ.iter().filter(|&&v| v != 0).count();
        let sg = fused.scene_grid();
        let m = surface_nets(&sg);
        // 高斯档（颜色取源像素；不做梯度拟合）
        let mut splats = build(&fit_views, &fit_rgbs, cell, scale_k);
        for s in splats.iter_mut() {
            s.alpha = alpha;
        }
        let n = splats.len();
        // 可微拟合：只更新颜色（位置/尺度/alpha 固定），梯度精确
        let mut ps0 = Vec::new();
        for (v, gt) in eval_views.iter().zip(&eval_rgbs) {
            let img = render(&splats, &v.cam, v.w, v.h, [0.0, 0.0, 0.0]);
            ps0.push(masked_psnr(&img, gt, &v.ids));
        }
        let mean0 = avg(&ps0);
        if fit > 0 {
            let sets: Vec<trit_parts::splat::FitView> = fit_views
                .iter()
                .zip(&fit_rgbs)
                .map(|(v, rgb)| (v.cam.clone(), v.w, v.h, rgb.clone(), v.ids.clone()))
                .collect();
            let mut fit0 = Vec::new();
            for (v, gt) in fit_views.iter().zip(&fit_rgbs) {
                let img = render(&splats, &v.cam, v.w, v.h, [0.0, 0.0, 0.0]);
                fit0.push(masked_psnr(&img, gt, &v.ids));
            }
            if densify_rounds > 0 {
                let depths: Vec<Vec<f32>> = fit_views.iter().map(|v| v.depth.clone()).collect();
                for r in 0..densify_rounds {
                    let (a, pr) = trit_parts::splat::densify(
                        &mut splats,
                        &sets,
                        &depths,
                        cell * 0.6,
                        densify_err,
                        0.02,
                        20000,
                    );
                    println!(
                        "  densify 第 {} 轮：+{a} / -{pr} → {} 个",
                        r + 1,
                        splats.len()
                    );
                    trit_parts::splat::fit_all(
                        &mut splats,
                        &sets,
                        trit_parts::splat::FitOpts {
                            iters: fit.max(1),
                            lr_color: fit_lr,
                            lr_pos,
                            lr_scale,
                            lr_alpha,
                            reg: fit_reg,
                        },
                        [0.0, 0.0, 0.0],
                    );
                }
            } else if fit_all_mode {
                trit_parts::splat::fit_all(
                    &mut splats,
                    &sets,
                    trit_parts::splat::FitOpts {
                        iters: fit,
                        lr_color: fit_lr,
                        lr_pos,
                        lr_scale,
                        lr_alpha,
                        reg: fit_reg,
                    },
                    [0.0, 0.0, 0.0],
                );
            } else {
                trit_parts::splat::fit_colors(&mut splats, &sets, fit, fit_lr, [0.0, 0.0, 0.0]);
            }
            let mut fit1 = Vec::new();
            for (v, gt) in fit_views.iter().zip(&fit_rgbs) {
                let img = render(&splats, &v.cam, v.w, v.h, [0.0, 0.0, 0.0]);
                fit1.push(masked_psnr(&img, gt, &v.ids));
            }
            println!("  拟合视角 PSNR {:.2} → {:.2}", avg(&fit0), avg(&fit1));
        }
        let mut ps = Vec::new();
        for (v, gt) in eval_views.iter().zip(&eval_rgbs) {
            let img = render(&splats, &v.cam, v.w, v.h, [0.0, 0.0, 0.0]);
            // 只在 GT 前景像素上算（地板/背景不在喷溅表示里）
            ps.push(masked_psnr(&img, gt, &v.ids));
            if let Some(dir) = arg(argv, "--dump") {
                let d = PathBuf::from(dir);
                std::fs::create_dir_all(&d).ok();
                let mut bytes = format!(
                    "P6
{} {}
255
",
                    v.w, v.h
                )
                .into_bytes();
                for p in 0..v.w * v.h {
                    for c in 0..3 {
                        bytes.push((img[p * 3 + c].clamp(0.0, 1.0) * 255.0) as u8);
                    }
                }
                let _ = std::fs::write(d.join(format!("splat_{cell:.3}.ppm")), bytes);
            }
        }
        let mean = avg(&ps);
        println!(
            "{cell:.3}     {vox:<9} {}/{}            {}            {n:<6}  {mean0:.2} → {mean:.2}",
            m.verts.len(),
            m.tris.len(),
            non_manifold_edges(&m)
        );
    }
}

/// 解析 `objects.json`（数组，元素含 kind/center/scale/rot）→ 原语列表。
fn parse_objects(path: &Path) -> Vec<trit_parts::occ::Primitive> {
    let text =
        std::fs::read_to_string(path).unwrap_or_else(|e| fail(&format!("{}: {e}", path.display())));
    let mut out = Vec::new();
    for rec in text.split("},") {
        let rec = rec.trim_start_matches('[').trim_end_matches(']');
        let field = |k: &str| -> Option<String> {
            let pat = format!("\"{k}\"");
            let at = rec.find(&pat)? + pat.len();
            let rest = &rec[at..];
            let colon = rest.find(':')?;
            Some(rest[colon + 1..].trim_start().to_string())
        };
        let kind = match field("kind") {
            Some(v) => v
                .trim_start_matches('"')
                .split('"')
                .next()
                .unwrap_or("")
                .to_string(),
            None => continue,
        };
        let nums = |v: &str| -> Vec<f32> {
            v.trim_start_matches('[')
                .split(']')
                .next()
                .unwrap_or("")
                .split(',')
                .filter_map(|x| x.trim().parse::<f32>().ok())
                .collect()
        };
        let center = field("center").map(|v| nums(&v)).unwrap_or_default();
        if center.len() != 3 {
            continue;
        }
        let scale = field("scale").map(|v| nums(&v)).unwrap_or_default();
        let rot = field("rot").map(|v| nums(&v)).unwrap_or_default();
        out.push(trit_parts::occ::Primitive {
            kind,
            center: [center[0], center[1], center[2]],
            scale: if scale.len() == 3 {
                [scale[0], scale[1], scale[2]]
            } else {
                [1.0; 3]
            },
            rot: if rot.len() == 3 {
                [rot[0], rot[1], rot[2]]
            } else {
                [0.0; 3]
            },
        });
    }
    out
}

/// `occ --root <renders 根> --scenes <scenes 根> --train 80,81 --eval 84,85`：
/// 学习式占据（三值 MLP）：训练用解析原语当 GT 实心，评测报体积 IoU 与网格闭合率。
fn occ_cmd(argv: &[String]) {
    use trit_parts::mesh::{boundary_edges, fix_diagonals, non_manifold_edges, surface_nets};
    use trit_parts::occ::{FPER, Mlp, Primitive, features, grid_for, selfsup_solid, solid};

    let root = PathBuf::from(arg(argv, "--root").unwrap_or_else(|| usage()));
    let scenes = PathBuf::from(arg(argv, "--scenes").unwrap_or_else(|| usage()));
    let parse_list = |k: &str, d: Vec<usize>| -> Vec<usize> {
        arg(argv, k)
            .map(|v| v.split(',').filter_map(|s| s.trim().parse().ok()).collect())
            .unwrap_or(d)
    };
    let train = parse_list("--train", Vec::new());
    let eval = parse_list("--eval", Vec::new());
    let mut frames = parse_list("--frames", vec![0, 1, 2, 3, 4, 5, 6, 7]);
    let n_views: usize = arg(argv, "--views")
        .and_then(|v| v.parse().ok())
        .unwrap_or(frames.len());
    frames.truncate(n_views.max(1));
    let cell: f32 = arg(argv, "--cell")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.01);
    let hid: usize = arg(argv, "--hid")
        .and_then(|v| v.parse().ok())
        .unwrap_or(16);
    let steps: usize = arg(argv, "--steps")
        .and_then(|v| v.parse().ok())
        .unwrap_or(20000);
    let lr: f32 = arg(argv, "--lr")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.05);
    let cap: usize = arg(argv, "--cap")
        .and_then(|v| v.parse().ok())
        .unwrap_or(20000);
    let save_model = arg(argv, "--save-model").map(PathBuf::from);
    let seal: usize = arg(argv, "--seal")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let selfsup = argv.iter().any(|a| a == "--selfsup");
    let tau: f32 = arg(argv, "--tau")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.01);
    if train.is_empty() || eval.is_empty() {
        fail("需要 --train 与 --eval（场景编号列表）");
    }

    let load_views = |idx: usize| -> Vec<trit_parts::fuse::View> {
        frames
            .iter()
            .map(|&f| {
                let d = root.join(format!("scene{idx:04}"));
                let (ids, shape) =
                    read_npy_i32(&frame_path(&d, "seg", f, "npy")).unwrap_or_else(|e| fail(&e));
                let (dep, _) =
                    read_npy_f32(&frame_path(&d, "depth", f, "npy")).unwrap_or_else(|e| fail(&e));
                trit_parts::fuse::View {
                    ids: ids.iter().map(|&v| v.max(0) as u32).collect(),
                    depth: dep,
                    w: shape[1],
                    h: shape[0],
                    cam: Camera::load(&frame_path(&d, "meta", f, "json"))
                        .unwrap_or_else(|e| fail(&e)),
                }
            })
            .collect()
    };

    // 训练：每场景平衡采样（实心/空各半）
    let nin = frames.len() * FPER + 1;
    let mut mlp = Mlp::new(nin, hid, 12345);
    let mut samples: Vec<(Vec<f32>, f32)> = Vec::new();
    for &idx in &train {
        let prims: Vec<Primitive> =
            parse_objects(&scenes.join(format!("scene{idx:04}/objects.json")));
        let views = load_views(idx);
        let g = if selfsup {
            let pts = view_points(&views);
            match trit_parts::mesh::Grid::from_points(&pts, cell, 3) {
                Some(g) => g,
                None => continue,
            }
        } else {
            match grid_for(&prims, cell, 2) {
                Some(g) => g,
                None => continue,
            }
        };
        let gt = if selfsup {
            selfsup_solid(&views, &g, tau)
        } else {
            solid(&prims, &g)
        };
        let mut pos = Vec::new();
        let mut neg = Vec::new();
        for (i, &label) in gt.iter().enumerate() {
            let (x, y, z) = (i % g.nx, (i / g.nx) % g.ny, i / (g.nx * g.ny));
            let p = [
                g.origin[0] + (x as f32 + 0.5) * g.cell,
                g.origin[1] + (y as f32 + 0.5) * g.cell,
                g.origin[2] + (z as f32 + 0.5) * g.cell,
            ];
            let f = features(&views, p);
            if label == 1 {
                pos.push((f, 1.0))
            } else {
                neg.push((f, 0.0))
            }
        }
        let k = pos.len().min(cap / 2).min(neg.len());
        samples.extend(pos.into_iter().take(k));
        samples.extend(neg.into_iter().take(k));
        println!("train scene{idx:04}: 采样 {k}+{k}");
    }
    if samples.is_empty() {
        fail("没有训练样本");
    }
    let mut loss_acc = 0.0f64;
    for step in 0..steps {
        let (x, y) = &samples[(step * 7919) % samples.len()];
        loss_acc += mlp.train_step(x, *y, lr) as f64;
        if (step + 1) % 2000 == 0 {
            println!(
                "  step {} loss(平均) {:.4} 三值零占比 {:.2}",
                step + 1,
                loss_acc / 2000.0,
                mlp.sparsity()
            );
            loss_acc = 0.0;
        }
    }

    if let Some(p) = &save_model {
        mlp.save(p).unwrap_or_else(|e| fail(&e));
        println!("模型已存 {}", p.display());
    }

    // 评测：体积 IoU（vs 原语实心）+ 网格闭合率
    let (mut ious, mut closed, mut n) = (Vec::new(), 0usize, 0usize);
    let mut lab_ious: Vec<f32> = Vec::new();
    for &idx in &eval {
        let prims: Vec<Primitive> =
            parse_objects(&scenes.join(format!("scene{idx:04}/objects.json")));
        let views = load_views(idx);
        let g = if selfsup {
            let pts = view_points(&views);
            match trit_parts::mesh::Grid::from_points(&pts, cell, 3) {
                Some(g) => g,
                None => continue,
            }
        } else {
            match grid_for(&prims, cell, 2) {
                Some(g) => g,
                None => continue,
            }
        };
        let gt = solid(&prims, &g); // 评测仍用解析实心（仅测量）
        let mut pred = g.clone();
        if selfsup {
            let lab = selfsup_solid(&views, &g, tau);
            let (mut li, mut lu) = (0usize, 0usize);
            for (a, b) in lab.iter().zip(gt.iter()) {
                let (a, b) = (*a != 0, *b != 0);
                li += (a && b) as usize;
                lu += (a || b) as usize;
            }
            let lv = li as f32 / lu.max(1) as f32;
            lab_ious.push(lv);
            println!("  [自监督标签 vs 解析实心] IoU {lv:.3}");
        }
        for z in 0..g.nz {
            for y in 0..g.ny {
                for x in 0..g.nx {
                    let p = [
                        g.origin[0] + (x as f32 + 0.5) * g.cell,
                        g.origin[1] + (y as f32 + 0.5) * g.cell,
                        g.origin[2] + (z as f32 + 0.5) * g.cell,
                    ];
                    let (zlogit, _) = mlp.forward(&features(&views, p));
                    pred.occ[x + g.nx * (y + g.ny * z)] = (zlogit > 0.0) as u8;
                }
            }
        }
        // 网格要闭合：先清掉贴着栅格边界的预测，再填实、修对角
        for z in 0..g.nz {
            for y in 0..g.ny {
                for x in 0..g.nx {
                    if x == 0 || y == 0 || z == 0 || x + 1 == g.nx || y + 1 == g.ny || z + 1 == g.nz
                    {
                        pred.occ[x + g.nx * (y + g.ny * z)] = 0;
                    }
                }
            }
        }
        if seal > 0 {
            trit_parts::mesh::dilate_occ(&mut pred, seal);
        }
        trit_parts::mesh::fill_solid(&mut pred);
        fix_diagonals(&mut pred);
        let (mut inter, mut union) = (0usize, 0usize);
        for (a, b) in pred.occ.iter().zip(gt.iter()) {
            let (a, b) = (*a != 0, *b != 0);
            inter += (a && b) as usize;
            union += (a || b) as usize;
        }
        let iou = if union > 0 {
            inter as f32 / union as f32
        } else {
            f32::NAN
        };
        let m = surface_nets(&pred);
        let ok = !m.tris.is_empty() && boundary_edges(&m) == 0;
        n += 1;
        closed += ok as usize;
        ious.push(iou);
        println!(
            "eval scene{idx:04}: 体积 IoU {iou:.3}  网格 顶点 {} 面 {} 边界边 {} 非流形 {} 闭合 {}",
            m.verts.len(),
            m.tris.len(),
            boundary_edges(&m),
            non_manifold_edges(&m),
            if ok { "是" } else { "否" }
        );
    }
    let mean = ious.iter().sum::<f32>() / ious.len().max(1) as f32;
    if !lab_ious.is_empty() {
        let lm = lab_ious.iter().sum::<f32>() / lab_ious.len() as f32;
        println!("标签 IoU 均值 {lm:.3}（{} 场景）", lab_ious.len());
    }
    println!(
        "N2 汇总：体积 IoU 均值 {mean:.3}（{} 场景）；闭合 {}/{}（{:.0}%）；门：IoU≥0.5 且 闭合率≥80%",
        ious.len(),
        closed,
        n,
        100.0 * closed as f64 / n.max(1) as f64
    );
}

fn view_points(views: &[trit_parts::fuse::View]) -> Vec<[f32; 3]> {
    let mut out = Vec::new();
    for v in views {
        for i in 0..v.ids.len().min(v.w * v.h) {
            let d = v.depth[i];
            if v.ids[i] == 0 || !(d.is_finite() && d > 0.0 && d < 50.0) {
                continue;
            }
            out.push(v.cam.unproject((i % v.w) as f64, (i / v.w) as f64, d));
        }
    }
    out
}

/// `align --scene <dir> --frames 0,1,... --cell 0.01`：网格（自监督实心）与高斯的一致性。
/// 报三项：高斯落在实心内的比例、落在表面 1 格内的比例、网格表面被高斯覆盖的比例。
fn align_cmd(argv: &[String]) {
    use trit_parts::occ::selfsup_solid;
    use trit_parts::splat::{build, collect};

    let scene = PathBuf::from(arg(argv, "--scene").unwrap_or_else(|| usage()));
    let frames: Vec<usize> = arg(argv, "--frames")
        .map(|v| v.split(',').filter_map(|s| s.trim().parse().ok()).collect())
        .unwrap_or_else(|| vec![0, 1, 2, 3, 4, 5, 6, 7]);
    let cell: f32 = arg(argv, "--cell")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.01);
    let tau: f32 = arg(argv, "--tau")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.01);

    let load = |f: usize| -> trit_parts::fuse::View {
        let (ids, shape) =
            read_npy_i32(&frame_path(&scene, "seg", f, "npy")).unwrap_or_else(|e| fail(&e));
        let (d, _) =
            read_npy_f32(&frame_path(&scene, "depth", f, "npy")).unwrap_or_else(|e| fail(&e));
        trit_parts::fuse::View {
            ids: ids.iter().map(|&v| v.max(0) as u32).collect(),
            depth: d,
            w: shape[1],
            h: shape[0],
            cam: Camera::load(&frame_path(&scene, "meta", f, "json")).unwrap_or_else(|e| fail(&e)),
        }
    };
    let views: Vec<_> = frames.iter().map(|&f| load(f)).collect();
    let rgbs: Vec<Vec<f32>> = frames
        .iter()
        .map(|&f| {
            trit_parts::data::read_rgb(&frame_path(&scene, "rgb", f, "png"))
                .map(|(_, _, p)| p)
                .unwrap_or_else(|e| fail(&e))
        })
        .collect();
    let pts = view_points(&views);
    let g = trit_parts::mesh::Grid::from_points(&pts, cell, 3).unwrap_or_else(|| fail("无有效点"));
    let solid = match arg(argv, "--model") {
        Some(p) => {
            let m = trit_parts::occ::Mlp::load(Path::new(&p)).unwrap_or_else(|e| fail(&e));
            model_solid(&m, &views, &g)
        }
        None => close_solid(&g, selfsup_solid(&views, &g, tau)),
    };
    let mut surface = vec![false; solid.len()];
    for z in 0..g.nz {
        for y in 0..g.ny {
            for x in 0..g.nx {
                let i = x + g.nx * (y + g.ny * z);
                if solid[i] == 0 {
                    continue;
                }
                let edge = [
                    (1i32, 0i32, 0i32),
                    (-1, 0, 0),
                    (0, 1, 0),
                    (0, -1, 0),
                    (0, 0, 1),
                    (0, 0, -1),
                ]
                .iter()
                .any(|&(dx, dy, dz)| {
                    let (xx, yy, zz) = (x as i32 + dx, y as i32 + dy, z as i32 + dz);
                    xx < 0
                        || yy < 0
                        || zz < 0
                        || xx as usize >= g.nx
                        || yy as usize >= g.ny
                        || zz as usize >= g.nz
                        || solid[xx as usize + g.nx * (yy as usize + g.ny * zz as usize)] == 0
                });
                surface[i] = edge;
            }
        }
    }
    let mut splats = build(&views, &rgbs, cell, 0.6);
    let fill = argv.iter().any(|a| a == "--fill");
    let cover_cells = |sp: &[trit_parts::splat::Splat], surface: &[bool]| -> usize {
        let mut covered = vec![false; surface.len()];
        for s in sp {
            let c = [0, 1, 2].map(|a| ((s.pos[a] - g.origin[a]) / g.cell).floor() as i64);
            if c.iter().any(|&v| v < 0)
                || c[0] as usize >= g.nx
                || c[1] as usize >= g.ny
                || c[2] as usize >= g.nz
            {
                continue;
            }
            let i = c[0] as usize + g.nx * (c[1] as usize + g.ny * c[2] as usize);
            if surface[i] {
                covered[i] = true;
            }
        }
        covered.iter().filter(|&&v| v).count()
    };
    let _ = &cover_cells;
    let mut inside = 0usize;
    let mut d1 = 0usize;
    let mut d2 = 0usize;
    let mut covered = vec![false; solid.len()];
    for s in &splats {
        let c = [0, 1, 2].map(|a| ((s.pos[a] - g.origin[a]) / g.cell).floor() as i64);
        if c.iter().any(|&v| v < 0)
            || c[0] as usize >= g.nx
            || c[1] as usize >= g.ny
            || c[2] as usize >= g.nz
        {
            continue;
        }
        let (x, y, z) = (c[0] as usize, c[1] as usize, c[2] as usize);
        let i = x + g.nx * (y + g.ny * z);
        if solid[i] != 0 {
            inside += 1;
        }
        if surface[i] {
            covered[i] = true;
        }
        let mut best = 3i32;
        for dz in -2i32..=2 {
            for dy in -2i32..=2 {
                for dx in -2i32..=2 {
                    let (xx, yy, zz) = (x as i32 + dx, y as i32 + dy, z as i32 + dz);
                    if xx < 0
                        || yy < 0
                        || zz < 0
                        || xx as usize >= g.nx
                        || yy as usize >= g.ny
                        || zz as usize >= g.nz
                    {
                        continue;
                    }
                    if surface[xx as usize + g.nx * (yy as usize + g.ny * zz as usize)] {
                        best = best.min(dx.abs() + dy.abs() + dz.abs());
                    }
                }
            }
        }
        if best <= 1 {
            d1 += 1;
        }
        if best <= 2 {
            d2 += 1;
        }
    }
    let n_surf = surface.iter().filter(|&&v| v).count();
    let n_cov = covered.iter().filter(|&&v| v).count();
    let n = splats.len().max(1) as f32;
    println!(
        "网格↔高斯（cell {cell:.3}，τ {tau:.3}，{} 视角）：高斯 {} 个",
        views.len(),
        splats.len()
    );
    println!(
        "  高斯在实心内 {:.1}%  |  距网格面 ≤1 格 {:.1}%  ≤2 格 {:.1}%",
        100.0 * inside as f32 / n,
        100.0 * d1 as f32 / n,
        100.0 * d2 as f32 / n
    );
    println!(
        "  网格表面被高斯覆盖 {:.1}%（{n_cov}/{n_surf}）",
        100.0 * n_cov as f32 / n_surf.max(1) as f32
    );
    // 第 0 视角：GS 覆盖像素里有多少落在前景（越低说明高斯漂到背景）
    let cw = collect(&splats, &views[0].cam, views[0].w, views[0].h);
    let (mut on_fg, mut cov_px) = (0usize, 0usize);
    for p in 0..views[0].w * views[0].h {
        if cw.coverage[p] > 0.5 {
            cov_px += 1;
            if views[0].ids[p] != 0 {
                on_fg += 1;
            }
        }
    }
    println!(
        "  第 0 视角：高斯覆盖像素 {cov_px}，其中落在前景 {:.1}%",
        100.0 * on_fg as f32 / cov_px.max(1) as f32
    );
    if fill {
        // 用实心表面未覆盖的格子补背面高斯（颜色取最近已观测高斯）
        let before_cov = cover_cells(&splats, &surface);
        let psnr_before = {
            let mut vs = Vec::new();
            for (v, gt) in views.iter().zip(&rgbs) {
                let img = trit_parts::splat::render(&splats, &v.cam, v.w, v.h, [0.0; 3]);
                vs.push(masked_psnr(&img, gt, &v.ids));
            }
            avg(&vs)
        };
        let added = trit_parts::splat::fill_unobserved(&mut splats, &g, &surface, cell * 0.6, 0.35);
        let after_cov = cover_cells(&splats, &surface);
        let psnr_after = {
            let mut vs = Vec::new();
            for (v, gt) in views.iter().zip(&rgbs) {
                let img = trit_parts::splat::render(&splats, &v.cam, v.w, v.h, [0.0; 3]);
                vs.push(masked_psnr(&img, gt, &v.ids));
            }
            avg(&vs)
        };
        println!(
            "  补背面：+{added} 个（alpha 0.35） → 表面覆盖 {:.1}% → {:.1}%（{n_surf} 格）；PSNR {psnr_before:.2} → {psnr_after:.2}",
            100.0 * before_cov as f32 / n_surf.max(1) as f32,
            100.0 * after_cov as f32 / n_surf.max(1) as f32
        );
    }
}

/// `obs-split --scene <dir> --frames ... --out <dir>`：把网格面按「是否被这些视角看到」拆成
/// 两份 PLY（`*.seen.ply` / `*.unseen.ply`），并打印可见面占比。
/// 判据：三角形重心投到某视角时落在前景，且深度与观测差 < 2×格。
fn obs_split_cmd(argv: &[String]) {
    use trit_parts::mesh::{
        boundary_edges, non_manifold_edges, surface_nets, write_ply as write_mesh,
    };
    use trit_parts::occ::selfsup_solid;

    let scene = PathBuf::from(arg(argv, "--scene").unwrap_or_else(|| usage()));
    let out = PathBuf::from(arg(argv, "--out").unwrap_or_else(|| usage()));
    let frames: Vec<usize> = arg(argv, "--frames")
        .map(|v| v.split(',').filter_map(|s| s.trim().parse().ok()).collect())
        .unwrap_or_else(|| vec![0, 1, 2, 3, 4, 5, 6, 7]);
    let cell: f32 = arg(argv, "--cell")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.01);
    let tau: f32 = arg(argv, "--tau")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.01);

    let load = |f: usize| -> trit_parts::fuse::View {
        let (ids, shape) =
            read_npy_i32(&frame_path(&scene, "seg", f, "npy")).unwrap_or_else(|e| fail(&e));
        let (d, _) =
            read_npy_f32(&frame_path(&scene, "depth", f, "npy")).unwrap_or_else(|e| fail(&e));
        trit_parts::fuse::View {
            ids: ids.iter().map(|&v| v.max(0) as u32).collect(),
            depth: d,
            w: shape[1],
            h: shape[0],
            cam: Camera::load(&frame_path(&scene, "meta", f, "json")).unwrap_or_else(|e| fail(&e)),
        }
    };
    let views: Vec<_> = frames.iter().map(|&f| load(f)).collect();
    let pts = view_points(&views);
    let g = trit_parts::mesh::Grid::from_points(&pts, cell, 3).unwrap_or_else(|| fail("无有效点"));
    let solid = match arg(argv, "--model") {
        Some(p) => {
            let m = trit_parts::occ::Mlp::load(Path::new(&p)).unwrap_or_else(|e| fail(&e));
            model_solid(&m, &views, &g)
        }
        None => close_solid(&g, selfsup_solid(&views, &g, tau)),
    };
    let sg = trit_parts::mesh::Grid {
        occ: solid.clone(),
        nx: g.nx,
        ny: g.ny,
        nz: g.nz,
        origin: g.origin,
        cell: g.cell,
    };
    let m = surface_nets(&sg);
    let mut seen = trit_parts::mesh::Mesh::default();
    let mut unseen = trit_parts::mesh::Mesh::default();
    let tol = 2.0 * cell;
    for t in &m.tris {
        let a = m.verts[t[0] as usize];
        let b = m.verts[t[1] as usize];
        let c = m.verts[t[2] as usize];
        let cen = [
            (a[0] + b[0] + c[0]) / 3.0,
            (a[1] + b[1] + c[1]) / 3.0,
            (a[2] + b[2] + c[2]) / 3.0,
        ];
        let mut ok = false;
        for v in &views {
            let Some((u, vv, d)) = v.cam.project(cen) else {
                continue;
            };
            if u < 0.0 || vv < 0.0 || u >= v.w as f64 || vv >= v.h as f64 {
                continue;
            }
            let i = vv as usize * v.w + u as usize;
            if v.ids[i] != 0 && (d - v.depth[i] as f64).abs() <= f64::from(tol) {
                ok = true;
                break;
            }
        }
        let dst = if ok { &mut seen } else { &mut unseen };
        let base = dst.verts.len() as u32;
        for p in [a, b, c] {
            dst.verts.push(p);
        }
        dst.tris.push([base, base + 1, base + 2]);
    }
    let stem = out.file_stem().and_then(|s| s.to_str()).unwrap_or("mesh");
    let ps = out.with_file_name(format!("{stem}.seen.ply"));
    let pu = out.with_file_name(format!("{stem}.unseen.ply"));
    write_mesh(&ps, &seen).unwrap_or_else(|e| fail(&e));
    write_mesh(&pu, &unseen).unwrap_or_else(|e| fail(&e));
    let n = m.tris.len().max(1) as f32;
    println!(
        "面 {}：可见 {}（{:.1}%）不可见 {}（{:.1}%）| 网格边界边 {} 非流形 {}",
        m.tris.len(),
        seen.tris.len(),
        100.0 * seen.tris.len() as f32 / n,
        unseen.tris.len(),
        100.0 * unseen.tris.len() as f32 / n,
        boundary_edges(&m),
        non_manifold_edges(&m)
    );
    println!("  → {} / {}", ps.display(), pu.display());
}

/// 闭合后处理：清边界 → 填实 → 修对角（自举标签与模型输出共用）。
fn close_solid(g: &trit_parts::mesh::Grid, occ: Vec<u8>) -> Vec<u8> {
    let mut pred = g.clone();
    pred.occ = occ;
    for z in 0..g.nz {
        for y in 0..g.ny {
            for x in 0..g.nx {
                if x == 0 || y == 0 || z == 0 || x + 1 == g.nx || y + 1 == g.ny || z + 1 == g.nz {
                    pred.occ[x + g.nx * (y + g.ny * z)] = 0;
                }
            }
        }
    }
    trit_parts::mesh::fill_solid(&mut pred);
    trit_parts::mesh::fix_diagonals(&mut pred);
    pred.occ
}

/// 用训练好的占据模型对整格推理，并做「清边界 → 填实 → 修对角」得到闭合实心。
fn model_solid(
    model: &trit_parts::occ::Mlp,
    views: &[trit_parts::fuse::View],
    g: &trit_parts::mesh::Grid,
) -> Vec<u8> {
    use trit_parts::occ::features;
    let mut pred = g.clone();
    for z in 0..g.nz {
        for y in 0..g.ny {
            for x in 0..g.nx {
                let p = [
                    g.origin[0] + (x as f32 + 0.5) * g.cell,
                    g.origin[1] + (y as f32 + 0.5) * g.cell,
                    g.origin[2] + (z as f32 + 0.5) * g.cell,
                ];
                let (zlogit, _) = model.forward(&features(views, p));
                pred.occ[x + g.nx * (y + g.ny * z)] = (zlogit > 0.0) as u8;
            }
        }
    }
    for z in 0..g.nz {
        for y in 0..g.ny {
            for x in 0..g.nx {
                if x == 0 || y == 0 || z == 0 || x + 1 == g.nx || y + 1 == g.ny || z + 1 == g.nz {
                    pred.occ[x + g.nx * (y + g.ny * z)] = 0;
                }
            }
        }
    }
    trit_parts::mesh::fill_solid(&mut pred);
    trit_parts::mesh::fix_diagonals(&mut pred);
    pred.occ
}

/// `mesh-tex --scene <dir> --frames ... --out <ply>`：网格 + 顶点颜色烘焙。
/// 颜色从最近的高斯取（观测面），没有近邻的顶点标灰（不可见/推断面）。
fn mesh_tex_cmd(argv: &[String]) {
    use trit_parts::mesh::{non_manifold_edges, surface_nets, write_ply_rgb};
    use trit_parts::occ::selfsup_solid;
    use trit_parts::splat::build;

    let scene = PathBuf::from(arg(argv, "--scene").unwrap_or_else(|| usage()));
    let out = PathBuf::from(arg(argv, "--out").unwrap_or_else(|| usage()));
    let frames: Vec<usize> = arg(argv, "--frames")
        .map(|v| v.split(',').filter_map(|s| s.trim().parse().ok()).collect())
        .unwrap_or_else(|| vec![0, 1, 2, 3, 4, 5, 6, 7]);
    let cell: f32 = arg(argv, "--cell")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.01);
    let tau: f32 = arg(argv, "--tau")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.01);

    let load = |f: usize| -> trit_parts::fuse::View {
        let (ids, shape) =
            read_npy_i32(&frame_path(&scene, "seg", f, "npy")).unwrap_or_else(|e| fail(&e));
        let (d, _) =
            read_npy_f32(&frame_path(&scene, "depth", f, "npy")).unwrap_or_else(|e| fail(&e));
        trit_parts::fuse::View {
            ids: ids.iter().map(|&v| v.max(0) as u32).collect(),
            depth: d,
            w: shape[1],
            h: shape[0],
            cam: Camera::load(&frame_path(&scene, "meta", f, "json")).unwrap_or_else(|e| fail(&e)),
        }
    };
    let views: Vec<_> = frames.iter().map(|&f| load(f)).collect();
    let rgbs: Vec<Vec<f32>> = frames
        .iter()
        .map(|&f| {
            trit_parts::data::read_rgb(&frame_path(&scene, "rgb", f, "png"))
                .map(|(_, _, p)| p)
                .unwrap_or_else(|e| fail(&e))
        })
        .collect();
    let pts = view_points(&views);
    let g = trit_parts::mesh::Grid::from_points(&pts, cell, 3).unwrap_or_else(|| fail("无有效点"));
    let solid = close_solid(&g, selfsup_solid(&views, &g, tau));
    // 最小壁厚：形态学开运算删掉薄板/相切壁（噪声网格上非流形边的源头）
    let solid = if let Some(r) = arg(argv, "--open").and_then(|v| v.parse::<usize>().ok()) {
        let before = solid.iter().filter(|&&v| v != 0).count();
        let s2 = trit_parts::mesh::open_solid(&solid, g.nx, g.ny, g.nz, r);
        let after = s2.iter().filter(|&&v| v != 0).count();
        println!(
            "开运算 r={r}：占用体素 {before}→{after}（略掉 {}，{:.2}%）",
            before.saturating_sub(after),
            100.0 * (before - after) as f32 / before.max(1) as f32
        );
        s2
    } else {
        solid
    };
    let sg = trit_parts::mesh::Grid {
        occ: solid,
        nx: g.nx,
        ny: g.ny,
        nz: g.nz,
        origin: g.origin,
        cell: g.cell,
    };
    let m = surface_nets(&sg);
    let m = if argv.iter().any(|a| a == "--clean") {
        let (c, st) = trit_parts::mesh::clean(&m);
        println!(
            "清理：面 {}→{}（退化 {}、重复 {}）| 非流形边 {}→{} | 边界边 {}→{} | 顶点 {}→{}",
            st.faces_before,
            st.faces_after,
            st.degenerate,
            st.duplicate,
            st.nonmanifold_before,
            st.nonmanifold_after,
            st.boundary_before,
            st.boundary_after,
            st.verts_before,
            st.verts_after
        );
        c
    } else {
        m
    };
    // 给了贴图（--tex）或 quilt 源（--quilt）就走 OBJ + MTL，否则维持原来的顶点色 PLY
    let quilt_p = arg(argv, "--quilt");
    let tex_p = arg(argv, "--tex");
    if let Some(obj_p) = arg(argv, "--obj") {
        let (tw, th, tpix) = if let Some(q) = &quilt_p {
            // quilt → stylize → atlas 的第一段：先从源图合成一张可贴的贴图
            let (sw, sh, src) =
                trit_parts::data::read_rgb(std::path::Path::new(q)).unwrap_or_else(|e| fail(&e));
            let n: usize = arg(argv, "--quilt-size")
                .and_then(|v| v.parse().ok())
                .unwrap_or(512);
            let patch: usize = arg(argv, "--patch")
                .and_then(|v| v.parse().ok())
                .unwrap_or(16);
            let ov: usize = arg(argv, "--overlap")
                .and_then(|v| v.parse().ok())
                .unwrap_or(patch / 4);
            let seed: u64 = arg(argv, "--quilt-seed")
                .and_then(|v| v.parse().ok())
                .unwrap_or(1);
            if patch > sw.min(sh) {
                fail(&format!("--patch {patch} 超过源图短边 {}", sw.min(sh)));
            }
            let out = trit_parts::tex::quilt(&src, sw, sh, n, n, patch, ov, seed);
            let l = trit_parts::look::analyze(&out, n, n);
            println!(
                "quilt：{}×{} 源 → {n}×{n}（patch {patch} 重叠 {ov} seed {seed}）stroke {:.2} coh {:.3}",
                sw, sh, l.stroke, l.coherence
            );
            (n, n, out)
        } else if let Some(t) = &tex_p {
            trit_parts::data::read_rgb(std::path::Path::new(t)).unwrap_or_else(|e| fail(&e))
        } else {
            fail("需要 --tex <贴图> 或 --quilt <源图> 之一");
        };
        // stylize → atlas 两段：贴图先过显式风格，再烘进图集（无风格参数时逐位不变）
        let tpix = match style_from_argv(argv) {
            Some(st) => {
                let before = trit_parts::look::analyze(&tpix, tw, th);
                let out = trit_parts::tex::stylize(&tpix, tw, th, &st);
                let after = trit_parts::look::analyze(&out, tw, th);
                println!(
                    "风格：contrast {:.3}→{:.3}，levels {:.0}→{:.0}，sat {:.3}→{:.3}，stroke {:.2}→{:.2}",
                    before.contrast,
                    after.contrast,
                    before.levels,
                    after.levels,
                    before.sat,
                    after.sat,
                    before.stroke,
                    after.stroke
                );
                out
            }
            None => tpix,
        };
        let tex_world: f32 = arg(argv, "--tex-world")
            .and_then(|v| v.parse().ok())
            .unwrap_or(cell * 40.0);
        let atlas_px: usize = arg(argv, "--atlas")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let obj_path = PathBuf::from(&obj_p);
        let stem = obj_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("mesh")
            .to_string();
        let png_p = obj_path.with_file_name(format!("{stem}.png"));
        if atlas_px > 0 {
            let texels: f32 = arg(argv, "--texels")
                .and_then(|v| v.parse().ok())
                .unwrap_or(1024.0);
            let ndeg: f32 = arg(argv, "--atlas-deg")
                .and_then(|v| v.parse().ok())
                .unwrap_or(45.0);
            let lscm = argv.iter().any(|a| a == "--lscm");
            // 按材质类别（面法线主轴 X/Y/Z）分别上风格：`--style-by-axis "cel:0.6,oil:0.5,-"`
            // `-` = 该类不处理；条目不足 3 个时其余为恒等。
            // 材质映射：manifest 的 pass_index → material；顶点用多视角投影投票定部件号
            let mat_spec = arg(argv, "--style-by-material");
            let per_class_data: Option<(Vec<u8>, Vec<trit_parts::tex::Style>)> = if let Some(spec) =
                mat_spec
            {
                use trit_parts::tex::{Preset, Style};
                const MATS: [&str; 4] = ["wood", "metal", "stone", "fabric"];
                let mut styles = vec![Style::default(); 5];
                for part in spec.split(',') {
                    let part = part.trim();
                    if part.is_empty() {
                        continue;
                    }
                    let (mat, st) = part.split_once('=').unwrap_or((part, "-"));
                    let k = MATS
                        .iter()
                        .position(|m| *m == mat)
                        .unwrap_or_else(|| fail(&format!("未知材质 {mat}（可选 {MATS:?}）")));
                    if st == "-" {
                        continue;
                    }
                    let (name, t) = st.split_once(':').unwrap_or((st, "1.0"));
                    let p = Preset::by_name(name).unwrap_or_else(|| {
                        fail(&format!("未知预设 {name}（可选 cel / oil / epic）"))
                    });
                    styles[k] = p.style(t.parse::<f32>().unwrap_or(1.0));
                }
                // 顶点 → 部件号（多视角投影投票）
                // 有 --material-model 时优先用分类器预测（真实数据没有 manifest 材质也能用）
                let pred = arg(argv, "--material-model").map(|mp| {
                    let mm = load_material_model(std::path::Path::new(&mp));
                    let min_px: usize = arg(argv, "--min-px")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(200);
                    let mut map: std::collections::HashMap<u32, usize> =
                        std::collections::HashMap::new();
                    for (id, feats) in part_features_in(&scene, 0, min_px) {
                        let sc = mm.scores(&feats);
                        let mut best = (f32::NEG_INFINITY, 0usize);
                        for (k, v) in sc.iter().enumerate() {
                            if *v > best.0 {
                                best = (*v, k);
                            }
                        }
                        map.insert(id, best.1);
                    }
                    println!("材质分类器：{} 个部件（帧 0）", map.len());
                    map
                });
                let txt =
                    std::fs::read_to_string(scene.join("manifest.json")).unwrap_or_else(|e| {
                        fail(&format!(
                            "读 manifest 失败（--style-by-material 需要它）：{e}"
                        ))
                    });
                let by_pass = materials_by_pass(&txt);
                let mat_of = |id: u32| -> u8 {
                    if let Some(p) = &pred
                        && let Some(k) = p.get(&id)
                    {
                        return *k as u8;
                    }
                    by_pass
                        .get(id as usize)
                        .and_then(|m| m.as_ref())
                        .and_then(|m| MATS.iter().position(|x| *x == m.as_str()))
                        .map(|k| k as u8)
                        .unwrap_or(4)
                };
                let vclass: Vec<u8> = m
                    .verts
                    .iter()
                    .map(|v| {
                        let mut votes: std::collections::HashMap<u32, usize> =
                            std::collections::HashMap::new();
                        for view in &views {
                            if let Some((u, vv, d)) = view.cam.project(*v) {
                                let (ui, vi) = (u.round(), vv.round());
                                if d > 0.0
                                    && ui >= 0.0
                                    && vi >= 0.0
                                    && (ui as usize) < view.w
                                    && (vi as usize) < view.h
                                {
                                    let id = view.ids[vi as usize * view.w + ui as usize];
                                    if id > 0 {
                                        *votes.entry(id).or_default() += 1;
                                    }
                                }
                            }
                        }
                        // 平票必须确定性：先比票数，再比部件号（否则 HashMap 顺序会让结果不可复现）
                        votes
                            .into_iter()
                            .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))
                            .map(|(id, _)| mat_of(id))
                            .unwrap_or(4)
                    })
                    .collect();
                let class: Vec<u8> = m
                    .tris
                    .iter()
                    .map(|t| {
                        let (a, b, c) = (
                            vclass[t[0] as usize],
                            vclass[t[1] as usize],
                            vclass[t[2] as usize],
                        );
                        if a == b || a == c {
                            a
                        } else if b == c {
                            b
                        } else {
                            4
                        }
                    })
                    .collect();
                let mut cnt = [0usize; 5];
                for &c in &class {
                    cnt[c as usize] += 1;
                }
                println!(
                    "按材质分风格：wood {} / metal {} / stone {} / fabric {} / 未知 {} 个三角形（{spec}）",
                    cnt[0], cnt[1], cnt[2], cnt[3], cnt[4]
                );
                Some((class, styles))
            } else {
                arg(argv, "--style-by-axis").map(|spec| {
                    use trit_parts::tex::{Preset, Style};
                    let mut styles: Vec<Style> = Vec::new();
                    for part in spec.split(',') {
                        let part = part.trim();
                        if part == "-" || part.is_empty() {
                            styles.push(Style::default());
                            continue;
                        }
                        let (name, t) = part.split_once(':').unwrap_or((part, "1.0"));
                        let p = Preset::by_name(name).unwrap_or_else(|| {
                            fail(&format!("未知预设 {name}（可选 cel / oil / epic 或 -）"))
                        });
                        styles.push(p.style(t.parse::<f32>().unwrap_or(1.0)));
                    }
                    while styles.len() < 3 {
                        styles.push(Style::default());
                    }
                    let class: Vec<u8> = m
                        .tris
                        .iter()
                        .map(|t| {
                            let a = m.verts[t[0] as usize];
                            let b = m.verts[t[1] as usize];
                            let c = m.verts[t[2] as usize];
                            let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                            let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
                            let n = [
                                (e1[1] * e2[2] - e1[2] * e2[1]).abs(),
                                (e1[2] * e2[0] - e1[0] * e2[2]).abs(),
                                (e1[0] * e2[1] - e1[1] * e2[0]).abs(),
                            ];
                            if n[0] >= n[1] && n[0] >= n[2] {
                                0u8
                            } else if n[1] >= n[2] {
                                1u8
                            } else {
                                2u8
                            }
                        })
                        .collect();
                    println!("按主轴分风格：X/Y/Z = {spec}");
                    (class, styles)
                })
            };
            let pc = per_class_data
                .as_ref()
                .map(|(c, st)| trit_parts::atlas::PerClass {
                    class: c,
                    styles: st,
                });
            let t0 = std::time::Instant::now();
            let a = trit_parts::atlas::bake_atlas(
                &m,
                &tpix,
                tw,
                th,
                atlas_px,
                texels,
                ndeg,
                lscm,
                pc.as_ref(),
            )
            .unwrap_or_else(|e| fail(&e));
            println!(
                "展开 {}：{:.1} s",
                if lscm { "LSCM" } else { "平面投影" },
                t0.elapsed().as_secs_f32()
            );
            trit_parts::data::write_rgb(&png_p, a.w, a.h, &a.img).unwrap_or_else(|e| fail(&e));
            trit_parts::uv::write_obj(&obj_path, &m, &a.uv, &format!("{stem}.png"))
                .unwrap_or_else(|e| fail(&e));
            let st = trit_parts::atlas::uv_stats(&m, &a);
            if !a.styled_texels.is_empty() {
                println!(
                    "按类改写纹素（按 --style-by-* 的类别序）= {:?}",
                    a.styled_texels
                );
            }
            println!(
                "UV 密度：中位 {:.0} texel²/单位²，p05 {:.0} / p95 {:.0}（spread {:.2}）",
                st.median, st.p05, st.p95, st.spread
            );
            println!(
                "OBJ {}：{} 顶点 / {} 三角形，**图集** {}×{}（{} 个 chart，法线聚类 {:.0}°{}）⇒ {}",
                obj_path.display(),
                m.verts.len(),
                m.tris.len(),
                a.w,
                a.h,
                a.charts.len(),
                ndeg,
                if lscm { "，LSCM" } else { "" },
                png_p.display()
            );
            return;
        }
        let uvs = trit_parts::uv::box_uvs(&m, tex_world);
        trit_parts::data::write_rgb(&png_p, tw, th, &tpix).unwrap_or_else(|e| fail(&e));
        trit_parts::uv::write_obj(&obj_path, &m, &uvs, &format!("{stem}.png"))
            .unwrap_or_else(|e| fail(&e));
        println!(
            "OBJ {}：{} 顶点 / {} 三角形，投影 UV，贴图 {}×{}（一张覆盖 {:.3} 世界单位）⇒ {}",
            obj_path.display(),
            m.verts.len(),
            m.tris.len(),
            tw,
            th,
            tex_world,
            png_p.display()
        );
        return;
    }
    let splats = build(&views, &rgbs, cell, 0.6);
    // 高斯按格建索引，顶点找半径 ≤3 格的最近颜色
    let key = |p: [f32; 3]| {
        [
            ((p[0] - g.origin[0]) / g.cell).floor() as i64,
            ((p[1] - g.origin[1]) / g.cell).floor() as i64,
            ((p[2] - g.origin[2]) / g.cell).floor() as i64,
        ]
    };
    let mut colmap: std::collections::HashMap<[i64; 3], [u8; 3]> = std::collections::HashMap::new();
    for sp in &splats {
        let c = [
            (sp.color[0].clamp(0.0, 1.0) * 255.0) as u8,
            (sp.color[1].clamp(0.0, 1.0) * 255.0) as u8,
            (sp.color[2].clamp(0.0, 1.0) * 255.0) as u8,
        ];
        colmap.insert(key(sp.pos), c);
    }
    let mut colors: Vec<Option<[u8; 3]>> = Vec::with_capacity(m.verts.len());
    let mut hit = 0usize;
    for v in &m.verts {
        let k = key(*v);
        let mut found = None;
        'search: for r in 0..=3i64 {
            for dz in -r..=r {
                for dy in -r..=r {
                    for dx in -r..=r {
                        if dx.abs().max(dy.abs()).max(dz.abs()) != r {
                            continue;
                        }
                        if let Some(c) = colmap.get(&[k[0] + dx, k[1] + dy, k[2] + dz]) {
                            found = Some(*c);
                            break 'search;
                        }
                    }
                }
            }
        }
        if found.is_some() {
            hit += 1;
        }
        colors.push(found);
    }
    write_ply_rgb(&out, &m, &colors).unwrap_or_else(|e| fail(&e));
    println!(
        "网格 {} 顶点 / {} 面（边界边 {} 非流形 {}）；顶点着色 {:.1}%（{} 个未观测标灰）→ {}",
        m.verts.len(),
        m.tris.len(),
        trit_parts::mesh::boundary_edges(&m),
        non_manifold_edges(&m),
        100.0 * hit as f32 / m.verts.len().max(1) as f32,
        m.verts.len() - hit,
        out.display()
    );
}

/// `tex --kind wood|marble|rust|grid --size 256 --out <ppm>`：程序化可平铺材质 + 无缝度。
fn tex_cmd(argv: &[String]) {
    let kind = arg(argv, "--kind").unwrap_or_else(|| "wood".into());
    let n: usize = arg(argv, "--size")
        .and_then(|v| v.parse().ok())
        .unwrap_or(256);
    let seed: u64 = arg(argv, "--seed")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let out = PathBuf::from(arg(argv, "--out").unwrap_or_else(|| usage()));
    let px = trit_parts::tex::tile(&kind, n, seed);
    let r = trit_parts::tex::seam_ratio(&px, n);
    let mut bytes = format!(
        "P6
{n} {n}
255
"
    )
    .into_bytes();
    for p in &px {
        bytes.extend_from_slice(p);
    }
    std::fs::write(&out, bytes).unwrap_or_else(|e| fail(&format!("{}: {e}", out.display())));
    println!(
        "{kind}: {n}×{n} 接缝比 {r:.2}（≈1 无缝）→ {}",
        out.display()
    );
}

/// `tex-prior [--patch 3|7] [--cond]`：N5 学习地基——用程序化材质当数据训三值小算子。
/// - 无 `--cond`：按像素回归（局部去噪），`nin = patch²`；
/// - 加 `--cond`：输入拼上材质 one-hot（4 维），验证「条件能否被学到」：
///   评测时同时报「正确条件」与「错误条件」的 PSNR，差得越多说明条件用得越实。
fn tex_prior_cmd(argv: &[String]) {
    use trit_parts::occ::Mlp;
    use trit_parts::tex::tile;

    let iters: usize = arg(argv, "--iters")
        .and_then(|v| v.parse().ok())
        .unwrap_or(30000);
    let sigma: f32 = arg(argv, "--sigma")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.1);
    let n: usize = arg(argv, "--size")
        .and_then(|v| v.parse().ok())
        .unwrap_or(64);
    let seed: u64 = arg(argv, "--seed")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let eval_seed: u64 = arg(argv, "--eval-seed")
        .and_then(|v| v.parse().ok())
        .unwrap_or(2);
    let _ = eval_seed; // --data 模式下评测用留出图片，不再需要 seed
    let lr: f32 = arg(argv, "--lr")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.03);
    let patch: usize = arg(argv, "--patch")
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);
    let cond = argv.iter().any(|a| a == "--cond");
    let r = (patch / 2) as i32;
    let kinds = ["wood", "marble", "rust", "grid"];
    let data_dir = arg(argv, "--data").map(PathBuf::from);
    let nin = patch * patch + if cond { kinds.len() } else { 0 };
    let hid: usize = arg(argv, "--hid")
        .and_then(|v| v.parse().ok())
        .unwrap_or(16);

    let gray = |kind: &str, sd: u64| -> Vec<f32> {
        tile(kind, n, sd)
            .iter()
            .map(|p| (0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32) / 255.0)
            .collect()
    };
    let mut rng: u64 = 0x9E37_79B9_7F4A_7C15 ^ seed;
    let mut nxt = || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        (rng >> 11) as f32 / (1u64 << 53) as f32
    };
    let gauss = |nxt: &mut dyn FnMut() -> f32| {
        let u1 = nxt().max(1e-6);
        let u2 = nxt();
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos()
    };
    let feats = |g: &[f32],
                 x: usize,
                 y: usize,
                 cond_idx: Option<usize>,
                 nxt: &mut dyn FnMut() -> f32|
     -> (Vec<f32>, f32) {
        let mut f = Vec::with_capacity(nin);
        for dy in -r..=r {
            for dx in -r..=r {
                let xx = ((x as i32 + dx).rem_euclid(n as i32)) as usize;
                let yy = ((y as i32 + dy).rem_euclid(n as i32)) as usize;
                f.push((g[yy * n + xx] + sigma * gauss(nxt)).clamp(0.0, 1.0));
            }
        }
        if let Some(ci) = cond_idx {
            for k in 0..kinds.len() {
                f.push((k == ci) as u8 as f32);
            }
        }
        (f, g[y * n + x])
    };

    // 数据源：--data <dir> 用真实材质（PNG，灰度，缩放到 n），否则用程序化材质
    let (images, names): (Vec<Vec<f32>>, Vec<String>) = match &data_dir {
        Some(dir) => {
            let mut v = Vec::new();
            let mut nm = Vec::new();
            let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
                .unwrap_or_else(|e| fail(&format!("读不了 {}: {e}", dir.display())))
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "png"))
                .collect();
            files.sort();
            for f in files {
                let (w0, h0, pix) = trit_parts::data::read_rgb(&f).unwrap_or_else(|e| fail(&e));
                let rgb = fit_rgb(w0, h0, &pix, n);
                let g: Vec<f32> = rgb
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .map(|c| 0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2])
                    .collect();
                nm.push(
                    f.file_stem()
                        .map(|x| x.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                );
                v.push(g);
            }
            if v.len() < 3 {
                fail("--data 至少要 3 张 PNG");
            }
            (v, nm)
        }
        None => (
            kinds.iter().map(|k| gray(k, seed)).collect(),
            kinds.iter().map(|k| (*k).to_string()).collect(),
        ),
    };
    // **固定留出**：默认按文件名排序取最前 2 张；扩数据集时新文件可能排到前面，
    // 所以跨规模实验要用 `--holdout-names` 显式钉住评测集（文件名前缀逗号分隔）。
    let hold_names: Vec<String> = arg(argv, "--holdout-names")
        .map(|v| {
            v.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let n_eval = 2usize.min(images.len() - 1);
    let (eval_idx, all_train_idx): (Vec<usize>, Vec<usize>) = if hold_names.is_empty() {
        ((0..n_eval).collect(), (n_eval..images.len()).collect())
    } else {
        let mut ev = Vec::new();
        let mut tr = Vec::new();
        for (i, nm) in names.iter().enumerate() {
            if hold_names.iter().any(|h| nm.starts_with(h.as_str())) {
                ev.push(i);
            } else {
                tr.push(i);
            }
        }
        if ev.is_empty() || tr.len() < 2 {
            fail("--holdout-names 没匹配到文件（或训练集不足）");
        }
        (ev, tr)
    };
    let train_cap: usize = arg(argv, "--train-imgs")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let train_idx: Vec<usize> = if train_cap > 0 && train_cap < all_train_idx.len() {
        all_train_idx[..train_cap].to_vec()
    } else {
        all_train_idx
    };

    let mut train: Vec<(Vec<f32>, f32)> = Vec::new();
    for (ki, &ii) in train_idx.iter().enumerate() {
        let g = &images[ii];
        for y in 0..n {
            for x in 0..n {
                let (f, t) = feats(
                    g,
                    x,
                    y,
                    if cond { Some(ki % kinds.len()) } else { None },
                    &mut nxt,
                );
                train.push((f, t));
            }
        }
    }
    let mut mlp = Mlp::new(nin, hid, seed);
    let mut loss = 0.0f64;
    for step in 0..iters {
        let (x, y) = &train[(step * 7919) % train.len()];
        loss += mlp.train_step(x, *y, lr) as f64;
        if (step + 1) % 5000 == 0 {
            println!(
                "  step {} loss {:.4} 三值零占比 {:.2}",
                step + 1,
                loss / 5000.0,
                mlp.sparsity()
            );
            loss = 0.0;
        }
    }

    let psnr_of = |mlp: &Mlp, g: &[f32], ci: Option<usize>, nxt: &mut dyn FnMut() -> f32| -> f64 {
        let (mut se_out, mut cnt) = (0.0f64, 0usize);
        for y in 0..n {
            for x in 0..n {
                let (f, clean) = feats(g, x, y, ci, nxt);
                let (logit, _) = mlp.forward(&f);
                let pred = 1.0 / (1.0 + (-logit).exp());
                se_out += f64::from((pred - clean).powi(2));
                cnt += 1;
            }
        }
        10.0 * (1.0 / (se_out / cnt as f64)).log10()
    };
    let mut good = Vec::new();
    let mut wrong = Vec::new();
    let eval_list: Vec<(String, &Vec<f32>)> = eval_idx
        .iter()
        .enumerate()
        .map(|(i, &ii)| {
            if data_dir.is_some() {
                (names[ii].clone(), &images[ii])
            } else {
                (format!("holdout{i}"), &images[ii])
            }
        })
        .collect();
    for (ki, (name, g)) in eval_list.iter().enumerate() {
        let g = *g;
        let p_ok = psnr_of(&mlp, g, if cond { Some(ki) } else { None }, &mut nxt);
        let p_bad = if cond {
            psnr_of(&mlp, g, Some((ki + 1) % kinds.len()), &mut nxt)
        } else {
            f64::NAN
        };
        println!(
            "留出 {name}: 正确条件 {p_ok:.2} dB{}",
            if cond {
                format!("  错误条件 {p_bad:.2} dB")
            } else {
                String::new()
            }
        );
        good.push(p_ok);
        if cond {
            wrong.push(p_bad);
        }
    }
    println!(
        "N5 学习地基：patch {}×{}，条件 {}，{} 步；留出 PSNR 均值 {:.2} dB{}",
        patch,
        patch,
        if cond { "开" } else { "关" },
        iters,
        avg(&good),
        if cond {
            format!(
                "；错条件 {:.2} dB（差 {:.2}）",
                avg(&wrong),
                avg(&good) - avg(&wrong)
            )
        } else {
            String::new()
        }
    );
}

/// `tex-conv --hidden 8,8 --iters 20000 [--cond]`：三值卷积栈（3×3、same、ReLU、sigmoid）去噪，
/// 与 `tex-prior` 的 3×3 MLP 基线在同一任务/同一协议下对照（留出材质种子、正确 vs 错误条件）。
fn tex_conv_cmd(argv: &[String]) {
    use trit_parts::tconv::Net;
    use trit_parts::tex::tile;

    let iters: usize = arg(argv, "--iters")
        .and_then(|v| v.parse().ok())
        .unwrap_or(20000);
    let sigma: f32 = arg(argv, "--sigma")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.1);
    let n: usize = arg(argv, "--size")
        .and_then(|v| v.parse().ok())
        .unwrap_or(64);
    let seed: u64 = arg(argv, "--seed")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let eval_seed: u64 = arg(argv, "--eval-seed")
        .and_then(|v| v.parse().ok())
        .unwrap_or(2);
    let lr: f32 = arg(argv, "--lr")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.05);
    let cond = argv.iter().any(|a| a == "--cond");
    let use_bn = argv.iter().any(|a| a == "--bn");
    let adam = argv.iter().any(|a| a == "--adam");
    let bsz: usize = arg(argv, "--batch")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let hidden: Vec<usize> = arg(argv, "--hidden")
        .map(|v| v.split(',').filter_map(|s| s.trim().parse().ok()).collect())
        .unwrap_or_else(|| vec![8, 8]);

    let kinds = ["wood", "marble", "rust", "grid"];
    let gray = |kind: &str, sd: u64| -> Vec<f32> {
        tile(kind, n, sd)
            .iter()
            .map(|p| (0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32) / 255.0)
            .collect()
    };
    let mut rng: u64 = 0x9E37_79B9_7F4A_7C15 ^ seed;
    let mut nxt = || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        (rng >> 11) as f32 / (1u64 << 53) as f32
    };
    let noisy = |clean: &[f32], nxt: &mut dyn FnMut() -> f32| -> Vec<f32> {
        clean
            .iter()
            .map(|v| {
                let u1 = nxt().max(1e-6);
                let u2 = nxt();
                let z = (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos();
                (v + sigma * z).clamp(0.0, 1.0)
            })
            .collect()
    };
    let cin = 1 + if cond { kinds.len() } else { 0 };
    let mut net = Net::new(cin, &hidden, seed);
    net.cond = if cond { kinds.len() } else { 0 };
    net.set_bn(use_bn);
    let mut loss = 0.0f64;
    for step in 0..iters {
        let mut batch = Vec::with_capacity(bsz);
        for b in 0..bsz {
            let ki = (step * 7919 + b * 104729) % kinds.len();
            let sd = seed + ki as u64 + (step / kinds.len()) as u64; // 同种类换种子
            let clean = gray(kinds[ki], sd);
            let inp = noisy(&clean, &mut nxt);
            batch.push((inp, clean, if cond { Some(ki) } else { None }));
        }
        loss += net.train_batch(&batch, n, n, lr, adam) as f64;
        if (step + 1) % 2000 == 0 {
            println!(
                "  step {} loss {:.4} | aux CE {:.3} acc {:.2}",
                step + 1,
                loss / 2000.0,
                net.aux_ce,
                net.aux_acc
            );
            loss = 0.0;
        }
    }
    // 参考图由**图像种类**决定，条件只影响模型输入（正确/错误条件对比必须保持参考不变）
    let eval = |net: &Net,
                img_kind: usize,
                cond_idx: Option<usize>,
                sd: u64,
                nxt: &mut dyn FnMut() -> f32|
     -> f64 {
        let clean = gray(kinds[img_kind], sd);
        let inp = noisy(&clean, nxt);
        let pred = net.forward(&inp, cond_idx, n, n);
        let (mut se, mut cnt) = (0.0f64, 0usize);
        for i in 0..n * n {
            se += f64::from((pred[i] - clean[i]).powi(2));
            cnt += 1;
        }
        10.0 * (1.0 / (se / cnt as f64)).log10()
    };
    let mut good = Vec::new();
    let mut wrong = Vec::new();
    for (ki, k) in kinds.iter().enumerate() {
        let sd = eval_seed + ki as u64;
        let p_ok = eval(&net, ki, if cond { Some(ki) } else { None }, sd, &mut nxt);
        let p_bad = if cond {
            eval(&net, ki, Some((ki + 1) % kinds.len()), sd, &mut nxt)
        } else {
            f64::NAN
        };
        println!("留出 {k}（seed {eval_seed}）: 正确条件 {p_ok:.2} dB  错误条件 {p_bad:.2} dB");
        good.push(p_ok);
        if cond {
            wrong.push(p_bad);
        }
    }
    println!(
        "三值卷积栈 hidden {:?}，BN {}，Adam {}，batch {}，条件 {}，{} 步；留出 PSNR 均值 {:.2} dB{}",
        hidden,
        if use_bn { "开" } else { "关" },
        if adam { "开" } else { "关" },
        bsz,
        if cond { "开" } else { "关" },
        iters,
        avg(&good),
        if cond {
            format!(
                "；错条件 {:.2} dB（差 {:.2}）",
                avg(&wrong),
                avg(&good) - avg(&wrong)
            )
        } else {
            String::new()
        }
    );
}

/// `tex-gen`：N5 生成第一块——DDIM 采样器 + 3×3 补丁去噪器，条件 = 材质 one-hot。
/// 训练在程序化材质上（随机像素 + 随机时间步），采样后用三项统计量对照真实材质：
/// 像素均值/方差、平铺接缝比、离最近真实补丁的距离（太小 = 抄训练集）。
/// tex-gen 里的三种去噪器；ε 模式要用**ε-空间的稳定采样器**，不能走 x0 形式的 DDIM。
enum Den {
    M(PatchMlp),
    C(Net),
    E(trit_parts::tconv::EpsDen),
    V(trit_parts::tconv::VDen),
}

impl Denoiser for Den {
    fn predict_x0(
        &self,
        xt: &[f32],
        t_frac: f32,
        cond: Option<usize>,
        w: usize,
        h: usize,
    ) -> Vec<f32> {
        match self {
            Den::M(d) => d.predict_x0(xt, t_frac, cond, w, h),
            Den::C(d) => d.predict_x0(xt, t_frac, cond, w, h),
            Den::E(d) => d.predict_x0(xt, t_frac, cond, w, h),
            Den::V(d) => d.predict_x0(xt, t_frac, cond, w, h),
        }
    }
    fn input_dim(&self) -> usize {
        match self {
            Den::M(d) => d.input_dim(),
            Den::C(d) => d.input_dim(),
            Den::E(d) => d.input_dim(),
            Den::V(d) => d.input_dim(),
        }
    }
}

impl Den {
    /// `eps_space=true` 走 ε-空间更新（高 t 系数放大误差，实测会饱和，仅作对照）。
    #[allow(clippy::too_many_arguments)]
    fn sample(
        &self,
        sched: &Schedule,
        w: usize,
        h: usize,
        seed: u64,
        cond: Option<usize>,
        eps_space: bool,
        clamp: bool,
    ) -> Vec<f32> {
        match self {
            Den::E(e) if eps_space => {
                trit_parts::tconv::ddim_sample_eps(&e.net, &e.abar, w, h, seed, e.eps_scale)
            }
            _ => trit_parts::diffuse::ddim_sample_opts(self, sched, cond, w, h, seed, clamp),
        }
    }
}

/// `tex-quilt <源.png> [--out out.ppm] [--size 128] [--patch 12] [--overlap 3] [--seed 1]`：
/// 非参数 patch 合成（搬真实 patch，结构/统计由构造保证；与 `tex-gen` 的学出来的合成对照用）。
/// look 的 22 个描述量（顺序固定；CSV 与训练都用它）。
const LOOK_FEATURES: [&str; 22] = [
    "luma",
    "contrast",
    "sat",
    "hue_entropy",
    "edge",
    "fine",
    "strong",
    "flat",
    "levels",
    "haze",
    "emissive",
    "bars",
    "warmth",
    "key_dir",
    "key_fill_dwarm",
    "highlights",
    "hf1",
    "hf2",
    "hf4",
    "border",
    "stroke",
    "coherence",
];

fn look_vec(l: &trit_parts::look::Look) -> Vec<f32> {
    vec![
        l.luma,
        l.contrast,
        l.sat,
        l.hue_entropy,
        l.edge,
        l.fine,
        l.strong,
        l.flat,
        l.levels,
        l.haze,
        l.emissive,
        l.bars,
        l.warmth,
        l.key_dir,
        l.key_fill_dwarm,
        l.highlights,
        l.hf1,
        l.hf2,
        l.hf4,
        l.border,
        l.stroke,
        l.coherence,
    ]
}

/// 取一帧里所有可见部件（像素数 ≥ min_px）的 22 维特征：`(部件号, 特征)`。
/// 掩码外填该部件均值色，避免背景污染特征。
fn part_features_in(scene: &Path, frame: usize, min_px: usize) -> Vec<(u32, Vec<f32>)> {
    let (ids, shape) = trit_parts::data::read_npy_i32(&frame_path(scene, "seg", frame, "npy"))
        .unwrap_or_else(|e| fail(&e));
    let (rgbw, rgbh, rgb) = trit_parts::data::read_rgb(&frame_path(scene, "rgb", frame, "png"))
        .unwrap_or_else(|e| fail(&e));
    let (h, w) = (shape[0], shape[1]);
    if (w, h) != (rgbw, rgbh) {
        fail("seg 与 rgb 尺寸不一致");
    }
    let mut present: std::collections::BTreeMap<u32, (usize, usize, usize, usize, usize)> =
        std::collections::BTreeMap::new();
    for y in 0..h {
        for x in 0..w {
            let id = ids[y * w + x];
            if id <= 0 {
                continue;
            }
            let e = present.entry(id as u32).or_insert((x, y, x, y, 0));
            e.0 = e.0.min(x);
            e.1 = e.1.min(y);
            e.2 = e.2.max(x);
            e.3 = e.3.max(y);
            e.4 += 1;
        }
    }
    let mut out = Vec::new();
    for (id, (x0, y0, x1, y1, cnt)) in present {
        if cnt < min_px {
            continue;
        }
        let mut mean = [0.0f32; 3];
        for y in y0..=y1 {
            for x in x0..=x1 {
                if ids[y * w + x] == id as i32 {
                    for c in 0..3 {
                        mean[c] += rgb[(y * w + x) * 3 + c];
                    }
                }
            }
        }
        for m in mean.iter_mut() {
            *m /= cnt as f32;
        }
        let (cw, ch) = (x1 - x0 + 1, y1 - y0 + 1);
        let mut crop = vec![0.0f32; cw * ch * 3];
        for y in 0..ch {
            for x in 0..cw {
                let inside = ids[(y0 + y) * w + x0 + x] == id as i32;
                for c in 0..3 {
                    crop[(y * cw + x) * 3 + c] = if inside {
                        rgb[((y0 + y) * w + x0 + x) * 3 + c]
                    } else {
                        mean[c]
                    };
                }
            }
        }
        let l = trit_parts::look::analyze(&crop, cw, ch);
        out.push((id, look_vec(&l)));
    }
    out
}

/// `material-data --scene <dir> --frames 0,1,2 --out csv [--min-px 200]`：
/// 按 seg 取每个部件的**掩码裁剪**，算 look 的 22 个描述量 + manifest 材质标签，写 CSV（训练用）。
fn material_data_cmd(argv: &[String]) {
    let scene = PathBuf::from(arg(argv, "--scene").unwrap_or_else(|| usage()));
    let out = PathBuf::from(arg(argv, "--out").unwrap_or_else(|| usage()));
    let frames: Vec<usize> = arg(argv, "--frames")
        .map(|v| v.split(',').filter_map(|s| s.trim().parse().ok()).collect())
        .unwrap_or_else(|| vec![0]);
    let min_px: usize = arg(argv, "--min-px")
        .and_then(|v| v.parse().ok())
        .unwrap_or(200);
    let txt = std::fs::read_to_string(scene.join("manifest.json"))
        .unwrap_or_else(|e| fail(&format!("{}: {e}", scene.display())));
    let by_pass = materials_by_pass(&txt);
    let mut csv = String::from("scene,frame,part,material");
    for f in LOOK_FEATURES {
        csv.push(',');
        csv.push_str(f);
    }
    csv.push('\n');
    let mut n = 0usize;
    for &fr in &frames {
        for (id, feats) in part_features_in(&scene, fr, min_px) {
            let Some(mat) = by_pass.get(id as usize).and_then(|m| m.clone()) else {
                continue;
            };
            let name = scene
                .file_name()
                .and_then(|x| x.to_str())
                .unwrap_or("scene")
                .to_string();
            csv.push_str(&format!("{name},{fr},{id},{mat}"));
            for v in feats {
                csv.push_str(&format!(",{v:.6}"));
            }
            csv.push('\n');
            n += 1;
        }
    }
    std::fs::write(&out, csv).unwrap_or_else(|e| fail(&format!("{}: {e}", out.display())));
    println!("material-data：{n} 个「帧×部件」样本 → {}", out.display());
}

/// `occl-stats --scene <dir> --frames 0,1,..,7 --cell 0.03`：
/// 时序/遮挡的**测量片**：每个部件在多少帧可见、最长连续遮挡、以及它的 3D 体素覆盖随可见帧数的变化。
fn occl_stats_cmd(argv: &[String]) {
    use std::collections::BTreeMap;
    let scene = PathBuf::from(arg(argv, "--scene").unwrap_or_else(|| usage()));
    let frames: Vec<usize> = arg(argv, "--frames")
        .map(|v| v.split(',').filter_map(|s| s.trim().parse().ok()).collect())
        .unwrap_or_else(|| vec![0, 1, 2, 3, 4, 5, 6, 7]);
    let cell: f32 = arg(argv, "--cell")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.03);
    let min_px: usize = arg(argv, "--min-px")
        .and_then(|v| v.parse().ok())
        .unwrap_or(50);
    // 每帧读 seg+depth+cam，统计可见性并把点体素化
    let mut vis: BTreeMap<u32, Vec<bool>> = BTreeMap::new();
    let mut vox: BTreeMap<u32, std::collections::HashSet<(i32, i32, i32)>> = BTreeMap::new();
    let nframes = frames.len();
    for &fr in &frames {
        let (ids, shape) = trit_parts::data::read_npy_i32(&frame_path(&scene, "seg", fr, "npy"))
            .unwrap_or_else(|e| fail(&e));
        let (depth, dshape) =
            trit_parts::geo::read_npy_f32(&frame_path(&scene, "depth", fr, "npy"))
                .unwrap_or_else(|e| fail(&e));
        let cam =
            Camera::load(&frame_path(&scene, "meta", fr, "json")).unwrap_or_else(|e| fail(&e));
        let (h, w) = (shape[0], shape[1]);
        if dshape.len() != shape.len() || dshape[0] != h || dshape[1] != w {
            fail("seg 与 depth 尺寸不一致");
        }
        let mut cnt: BTreeMap<u32, usize> = BTreeMap::new();
        for i in 0..w * h {
            let id = ids[i];
            if id <= 0 {
                continue;
            }
            *cnt.entry(id as u32).or_default() += 1;
            let d = depth[i];
            if d.is_finite() && d > 0.0 && d < 50.0 {
                let p = cam.unproject((i % w) as f64, (i / w) as f64, d);
                let k = (
                    (p[0] / cell).floor() as i32,
                    (p[1] / cell).floor() as i32,
                    (p[2] / cell).floor() as i32,
                );
                vox.entry(id as u32).or_default().insert(k);
            }
        }
        for (id, c) in cnt {
            if c >= min_px {
                let e = vis.entry(id).or_insert_with(|| vec![false; nframes]);
                e[frames.iter().position(|&f| f == fr).unwrap_or(0)] = true;
            }
        }
    }
    // 对齐：没出现在任何帧的部件不统计
    let mut rows: Vec<(u32, usize, usize, usize)> = Vec::new(); // (id, 可见帧数, 最长遮挡, 体素数)
    for (id, v) in &vis {
        let seen = v.iter().filter(|x| **x).count();
        // 最长连续不可见（按帧序列，不跨首尾）
        let (mut best, mut cur) = (0usize, 0usize);
        for &b in v.iter() {
            if b {
                cur = 0;
            } else {
                cur += 1;
                best = best.max(cur);
            }
        }
        rows.push((*id, seen, best, vox.get(id).map(|s| s.len()).unwrap_or(0)));
    }
    rows.sort_by_key(|r| r.0);
    println!(
        "场景 {}：{} 帧，cell {:.3}，可见部件 {} 个",
        scene.file_name().and_then(|x| x.to_str()).unwrap_or("?"),
        nframes,
        cell,
        rows.len()
    );
    println!(
        "{:>5} {:>7} {:>9} {:>8} {:>9}",
        "部件", "可见帧", "最长遮挡", "体素数", "体素/帧"
    );
    let mut occluded = 0usize;
    for (id, seen, gap, nv) in &rows {
        if *seen < nframes {
            occluded += 1;
        }
        println!(
            "{:>5} {:>7} {:>9} {:>8} {:>9.0}",
            id,
            format!("{seen}/{nframes}"),
            gap,
            nv,
            *nv as f32 / (*seen).max(1) as f32
        );
    }
    let full: Vec<&(u32, usize, usize, usize)> = rows.iter().filter(|r| r.1 == nframes).collect();
    let part: Vec<&(u32, usize, usize, usize)> = rows.iter().filter(|r| r.1 < nframes).collect();
    let mean = |v: &[&(u32, usize, usize, usize)]| -> f32 {
        if v.is_empty() {
            0.0
        } else {
            v.iter().map(|r| r.3 as f32).sum::<f32>() / v.len() as f32
        }
    };
    println!(
        "遮挡率 {}/{} = {:.0}%；体素均值：全程可见 {:.0}（n={}）vs 曾被遮挡 {:.0}（n={}）",
        occluded,
        rows.len(),
        100.0 * occluded as f32 / rows.len().max(1) as f32,
        mean(&full),
        full.len(),
        mean(&part),
        part.len()
    );
}

/// 取一帧里每个可见部件（≥ min_px）的**方形裁剪**（掩码外填均值色），缩放到 `side×side`。
fn part_crops_in(
    scene: &Path,
    frame: usize,
    min_px: usize,
    side: usize,
) -> Vec<(u32, Vec<Vec<f32>>)> {
    let (ids, shape) = trit_parts::data::read_npy_i32(&frame_path(scene, "seg", frame, "npy"))
        .unwrap_or_else(|e| fail(&e));
    let (rgbw, rgbh, rgb) = trit_parts::data::read_rgb(&frame_path(scene, "rgb", frame, "png"))
        .unwrap_or_else(|e| fail(&e));
    let (h, w) = (shape[0], shape[1]);
    if (w, h) != (rgbw, rgbh) {
        fail("seg 与 rgb 尺寸不一致");
    }
    let mut present: std::collections::BTreeMap<u32, (usize, usize, usize, usize, usize)> =
        std::collections::BTreeMap::new();
    for y in 0..h {
        for x in 0..w {
            let id = ids[y * w + x];
            if id <= 0 {
                continue;
            }
            let e = present.entry(id as u32).or_insert((x, y, x, y, 0));
            e.0 = e.0.min(x);
            e.1 = e.1.min(y);
            e.2 = e.2.max(x);
            e.3 = e.3.max(y);
            e.4 += 1;
        }
    }
    let mut out = Vec::new();
    for (id, (x0, y0, x1, y1, cnt)) in present {
        if cnt < min_px {
            continue;
        }
        let mut mean = [0.0f32; 3];
        for y in y0..=y1 {
            for x in x0..=x1 {
                if ids[y * w + x] == id as i32 {
                    for c in 0..3 {
                        mean[c] += rgb[(y * w + x) * 3 + c];
                    }
                }
            }
        }
        for m in mean.iter_mut() {
            *m /= cnt as f32;
        }
        let (cw, ch) = (x1 - x0 + 1, y1 - y0 + 1);
        let mut planes = vec![vec![0.0f32; side * side]; 3];
        for yy in 0..side {
            for xx in 0..side {
                let sy = y0 + yy * ch / side;
                let sx = x0 + xx * cw / side;
                let inside = ids[sy * w + sx] == id as i32;
                for c in 0..3 {
                    planes[c][yy * side + xx] = if inside {
                        rgb[(sy * w + sx) * 3 + c]
                    } else {
                        mean[c]
                    };
                }
            }
        }
        out.push((id, planes));
    }
    out
}

/// `material-cnn --renders <dir> --holdout-every 5 --epochs 6 --lr 0.01 --out model.mcn`：
/// 三值卷积头（`matnet`）训练 + 按场景留出评估；与手工特征 + 岭回归（§6.64）同数据同协议对照。
fn material_cnn_cmd(argv: &[String]) {
    use trit_parts::matnet::{MatClass, MatNet, SIDE};
    let renders = PathBuf::from(arg(argv, "--renders").unwrap_or_else(|| usage()));
    let every: usize = arg(argv, "--holdout-every")
        .and_then(|v| v.parse().ok())
        .unwrap_or(5);
    let epochs: usize = arg(argv, "--epochs")
        .and_then(|v| v.parse().ok())
        .unwrap_or(6);
    let lr: f32 = arg(argv, "--lr")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.01);
    let hidden: usize = arg(argv, "--hidden")
        .and_then(|v| v.parse().ok())
        .unwrap_or(8);
    let frames: usize = arg(argv, "--frames-per-scene")
        .and_then(|v| v.parse().ok())
        .unwrap_or(4);
    let min_px: usize = arg(argv, "--min-px")
        .and_then(|v| v.parse().ok())
        .unwrap_or(200);
    let mut scenes: Vec<PathBuf> = std::fs::read_dir(&renders)
        .unwrap_or_else(|e| fail(&format!("{}: {e}", renders.display())))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir() && p.join("manifest.json").exists())
        .collect();
    scenes.sort();
    if scenes.is_empty() {
        fail("没有找到场景（需要 manifest.json）");
    }
    // 收集样本
    let mut samples: Vec<(bool, usize, Vec<Vec<f32>>)> = Vec::new(); // (是否留出, 类别, 输入)
    for (si, sc) in scenes.iter().enumerate() {
        let txt = std::fs::read_to_string(sc.join("manifest.json"))
            .unwrap_or_else(|e| fail(&format!("{e}")));
        let by_pass = materials_by_pass(&txt);
        let is_test = si % every == 0;
        for fr in 0..frames {
            for (id, planes) in part_crops_in(sc, fr, min_px, SIDE) {
                let Some(mat) = by_pass.get(id as usize).and_then(|m| m.clone()) else {
                    continue;
                };
                let Some(cls) = MatClass::from_name(&mat) else {
                    continue;
                };
                samples.push((is_test, cls as usize, planes));
            }
        }
    }
    let ntr = samples.iter().filter(|s| !s.0).count();
    let nte = samples.iter().filter(|s| s.0).count();
    if ntr == 0 || nte == 0 {
        fail(&format!(
            "样本不足：训练 {ntr} / 留出 {nte}（调 --holdout-every 或 --frames-per-scene）"
        ));
    }
    println!("样本 {ntr} 训练 / {nte} 留出（{SIDE}² 输入，{hidden} 通道三值卷积 ×2）");
    let seed: u64 = arg(argv, "--seed")
        .and_then(|v| v.parse().ok())
        .unwrap_or(7);
    let mut net = MatNet::new(hidden, seed);
    let mut order: Vec<usize> = (0..samples.len()).filter(|&i| !samples[i].0).collect();
    // 确定性打乱（LCG，不用系统随机）
    let mut st = seed | 1;
    for i in (1..order.len()).rev() {
        st ^= st << 13;
        st ^= st >> 7;
        st ^= st << 17;
        order.swap(i, (st as usize) % (i + 1));
    }
    for ep in 0..epochs {
        let mut loss = 0.0f32;
        for &i in &order {
            let (_, cls, planes) = &samples[i];
            loss += net.train_step(planes, SIDE, SIDE, *cls, lr);
        }
        println!(
            "epoch {}/{}：平均 CE {:.4}",
            ep + 1,
            epochs,
            loss / order.len() as f32
        );
    }
    let mut conf = [[0usize; 4]; 4];
    for s in &samples {
        if !s.0 {
            continue;
        }
        let (k, _) = net.predict(&s.2, SIDE, SIDE);
        conf[s.1][k] += 1;
    }
    print!("{:>8}", "真/预测");
    for c in MatClass::ALL {
        print!("{:>8}", c.name());
    }
    println!();
    let (mut correct, mut total) = (0usize, 0usize);
    for (k, row) in conf.iter().enumerate() {
        print!("{:>8}", MatClass::ALL[k].name());
        for v in row.iter() {
            print!("{:>8}", v);
        }
        println!();
        correct += row[k];
        total += row.iter().sum::<usize>();
    }
    println!(
        "三值卷积留出准确率 {correct}/{total} = {:.1}%",
        100.0 * correct as f32 / total.max(1) as f32
    );
    if let Some(p) = arg(argv, "--out") {
        net.save(std::path::Path::new(&p))
            .unwrap_or_else(|e| fail(&e));
        println!("模型写出 {p}");
    }
}

/// 材质模型（`material-fit --out` 写出的文本）。
struct MaterialModel {
    mu: Vec<f32>,
    sd: Vec<f32>,
    /// 每类权重：`w[k][0]` 是截距，其后是特征权重。
    w: Vec<Vec<f32>>,
}

fn load_material_model(path: &Path) -> MaterialModel {
    let txt =
        std::fs::read_to_string(path).unwrap_or_else(|e| fail(&format!("{}: {e}", path.display())));
    let mut mu = Vec::new();
    let mut sd = Vec::new();
    let mut w: Vec<Vec<f32>> = Vec::new();
    for line in txt.lines() {
        let p: Vec<&str> = line.split_whitespace().collect();
        match p.first().copied() {
            Some("mu") if p.len() >= 3 => mu.push(p[2].parse().unwrap_or(0.0)),
            Some("sd") if p.len() >= 3 => sd.push(p[2].parse().unwrap_or(1.0)),
            Some("w") if p.len() >= 4 => {
                let (k, j, v): (usize, usize, f32) = (
                    p[1].parse().unwrap_or(0),
                    p[2].parse().unwrap_or(0),
                    p[3].parse().unwrap_or(0.0),
                );
                while w.len() <= k {
                    w.push(Vec::new());
                }
                if w[k].len() <= j {
                    w[k].resize(j + 1, 0.0);
                }
                w[k][j] = v;
            }
            _ => {}
        }
    }
    if mu.is_empty() || w.is_empty() {
        fail("模型文件不完整（缺 mu 或 w）");
    }
    MaterialModel { mu, sd, w }
}

impl MaterialModel {
    fn scores(&self, x: &[f32]) -> Vec<f32> {
        self.w
            .iter()
            .map(|wk| {
                let mut s = wk.first().copied().unwrap_or(0.0);
                for (j, v) in x.iter().enumerate() {
                    let z = (v - self.mu[j]) / self.sd[j].max(1e-6);
                    s += wk.get(j + 1).copied().unwrap_or(0.0) * z;
                }
                s
            })
            .collect()
    }
}

/// `material-predict --model <txt> --scene <dir> [--frames 0] [--min-px 200] [--out csv]`：
/// 逐部件猜材质（真实数据没有 manifest 时用）；打印每部件预测与 margin。
fn material_predict_cmd(argv: &[String]) {
    const MATS: [&str; 4] = ["wood", "metal", "stone", "fabric"];
    let model = PathBuf::from(arg(argv, "--model").unwrap_or_else(|| usage()));
    let scene = PathBuf::from(arg(argv, "--scene").unwrap_or_else(|| usage()));
    let frames: Vec<usize> = arg(argv, "--frames")
        .map(|v| v.split(',').filter_map(|s| s.trim().parse().ok()).collect())
        .unwrap_or_else(|| vec![0]);
    let min_px: usize = arg(argv, "--min-px")
        .and_then(|v| v.parse().ok())
        .unwrap_or(200);
    let m = load_material_model(&model);
    let mut csv = String::from("frame,part,material,margin\n");
    for &fr in &frames {
        for (id, feats) in part_features_in(&scene, fr, min_px) {
            let sc = m.scores(&feats);
            let mut best = (f32::NEG_INFINITY, 0usize);
            let mut second = f32::NEG_INFINITY;
            for (k, v) in sc.iter().enumerate() {
                if *v > best.0 {
                    second = best.0;
                    best = (*v, k);
                } else if *v > second {
                    second = *v;
                }
            }
            println!(
                "帧 {fr} 部件 {id}: {} (margin {:.3})",
                MATS[best.1],
                best.0 - second
            );
            csv.push_str(&format!(
                "{fr},{id},{},{:.4}\n",
                MATS[best.1],
                best.0 - second
            ));
        }
    }
    if let Some(p) = arg(argv, "--out") {
        std::fs::write(&p, csv).unwrap_or_else(|e| fail(&format!("{p}: {e}")));
        println!("写出 {p}");
    }
}

/// `material-fit --csv <csv> --holdout-every 5 --lambda 1 [--out model.txt]`：
/// 按**场景**分组留出（每第 k 个场景留出，避免同场景泄漏），特征 z-score 后做一对多岭回归，报混淆矩阵。
fn material_fit_cmd(argv: &[String]) {
    use trit_parts::ridge;
    let csvp = PathBuf::from(arg(argv, "--csv").unwrap_or_else(|| usage()));
    let every: usize = arg(argv, "--holdout-every")
        .and_then(|v| v.parse().ok())
        .unwrap_or(5);
    let lambda: f32 = arg(argv, "--lambda")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1.0);
    let txt = std::fs::read_to_string(&csvp)
        .unwrap_or_else(|e| fail(&format!("{}: {e}", csvp.display())));
    const MATS: [&str; 4] = ["wood", "metal", "stone", "fabric"];
    let mut scenes: Vec<String> = Vec::new();
    let mut xs: Vec<Vec<f32>> = Vec::new();
    let mut ys: Vec<usize> = Vec::new();
    for (i, line) in txt.lines().enumerate() {
        if i == 0 || line.trim().is_empty() {
            continue;
        }
        let cols: Vec<&str> = line.split(',').collect();
        if cols.len() < 4 + LOOK_FEATURES.len() {
            continue;
        }
        let Some(k) = MATS.iter().position(|m| *m == cols[3]) else {
            continue;
        };
        xs.push(
            cols[4..4 + LOOK_FEATURES.len()]
                .iter()
                .map(|v| v.parse::<f32>().unwrap_or(0.0))
                .collect(),
        );
        ys.push(k);
        scenes.push(cols[0].to_string());
    }
    if xs.is_empty() {
        fail("CSV 里没有可用样本");
    }
    // --test-csv 给了就用「全量训练 + 外部测试集」；否则按场景分组留出
    if let Some(tp) = arg(argv, "--test-csv") {
        let ttxt = std::fs::read_to_string(&tp).unwrap_or_else(|e| fail(&format!("{tp}: {e}")));
        let mut txs: Vec<Vec<f32>> = Vec::new();
        let mut tys: Vec<usize> = Vec::new();
        for (i, line) in ttxt.lines().enumerate() {
            if i == 0 || line.trim().is_empty() {
                continue;
            }
            let cols: Vec<&str> = line.split(',').collect();
            if cols.len() < 4 + LOOK_FEATURES.len() {
                continue;
            }
            let Some(k) = MATS.iter().position(|m| *m == cols[3]) else {
                continue;
            };
            txs.push(
                cols[4..4 + LOOK_FEATURES.len()]
                    .iter()
                    .map(|v| v.parse::<f32>().unwrap_or(0.0))
                    .collect(),
            );
            tys.push(k);
        }
        if txs.is_empty() {
            fail("--test-csv 里没有可用样本");
        }
        let d = xs[0].len();
        let mut mu = vec![0.0f32; d];
        let mut sd = vec![0.0f32; d];
        for v in &xs {
            for j in 0..d {
                mu[j] += v[j];
            }
        }
        for m in mu.iter_mut() {
            *m /= xs.len() as f32;
        }
        for v in &xs {
            for j in 0..d {
                sd[j] += (v[j] - mu[j]).powi(2);
            }
        }
        for v in sd.iter_mut() {
            *v = (*v / xs.len() as f32).sqrt().max(1e-6);
        }
        let norm = |v: &Vec<f32>| -> Vec<f32> { (0..d).map(|j| (v[j] - mu[j]) / sd[j]).collect() };
        let xtr: Vec<Vec<f32>> = xs.iter().map(norm).collect();
        let mut head: Vec<Vec<f32>> = Vec::new();
        for k in 0..MATS.len() {
            let y: Vec<f32> = ys
                .iter()
                .map(|&t| if t == k { 1.0 } else { -1.0 })
                .collect();
            head.push(ridge::fit_ridge(&xtr, &y, lambda));
        }
        let mut conf = [[0usize; 4]; 4];
        for (v, &t) in txs.iter().zip(&tys) {
            let xn = norm(v);
            let mut best = (f32::NEG_INFINITY, 0usize);
            for (k, w) in head.iter().enumerate() {
                let sc = ridge::predict(w, &xn);
                if sc > best.0 {
                    best = (sc, k);
                }
            }
            conf[t][best.1] += 1;
        }
        println!("训练 {}（本批）/ 测试 {}（外部）", xs.len(), txs.len());
        print!("{:>8}", "真/预测");
        for m in MATS {
            print!("{:>8}", m);
        }
        println!();
        let (mut correct, mut total) = (0usize, 0usize);
        for (k, row) in conf.iter().enumerate() {
            print!("{:>8}", MATS[k]);
            for v in row.iter() {
                print!("{:>8}", v);
            }
            println!();
            correct += row[k];
            total += row.iter().sum::<usize>();
        }
        println!(
            "跨批准确率 {correct}/{total} = {:.1}%",
            100.0 * correct as f32 / total.max(1) as f32
        );
        return;
    }
    let mut order: Vec<String> = Vec::new();
    for s in &scenes {
        if !order.contains(s) {
            order.push(s.clone());
        }
    }
    let idx_of = |s: &str| order.iter().position(|x| x == s).unwrap_or(0);
    let is_test: Vec<bool> = scenes.iter().map(|s| idx_of(s) % every == 0).collect();
    let tr: Vec<usize> = (0..xs.len()).filter(|&i| !is_test[i]).collect();
    let te: Vec<usize> = (0..xs.len()).filter(|&i| is_test[i]).collect();
    if tr.is_empty() || te.is_empty() {
        fail("留出集或训练集为空：调大样本或改 --holdout-every");
    }
    let d = xs[0].len();
    let mut mu = vec![0.0f32; d];
    let mut sd = vec![0.0f32; d];
    for &i in &tr {
        for j in 0..d {
            mu[j] += xs[i][j];
        }
    }
    for m in mu.iter_mut() {
        *m /= tr.len() as f32;
    }
    for &i in &tr {
        for j in 0..d {
            sd[j] += (xs[i][j] - mu[j]).powi(2);
        }
    }
    for v in sd.iter_mut() {
        *v = (*v / tr.len() as f32).sqrt().max(1e-6);
    }
    let norm = |v: &Vec<f32>| -> Vec<f32> { (0..d).map(|j| (v[j] - mu[j]) / sd[j]).collect() };
    let xtr: Vec<Vec<f32>> = tr.iter().map(|&i| norm(&xs[i])).collect();
    let mut head: Vec<Vec<f32>> = Vec::new();
    for k in 0..MATS.len() {
        let y: Vec<f32> = tr
            .iter()
            .map(|&i| if ys[i] == k { 1.0 } else { -1.0 })
            .collect();
        head.push(ridge::fit_ridge(&xtr, &y, lambda));
    }
    let mut conf = [[0usize; 4]; 4];
    for &i in &te {
        let xn = norm(&xs[i]);
        let mut best = (f32::NEG_INFINITY, 0usize);
        for (k, w) in head.iter().enumerate() {
            let sc = ridge::predict(w, &xn);
            if sc > best.0 {
                best = (sc, k);
            }
        }
        conf[ys[i]][best.1] += 1;
    }
    println!(
        "样本 {}（训练 {} / 留出 {}，按场景留出 every={every}）",
        xs.len(),
        tr.len(),
        te.len()
    );
    print!("{:>8}", "真/预测");
    for m in MATS {
        print!("{:>8}", m);
    }
    println!();
    let (mut correct, mut total) = (0usize, 0usize);
    for k in 0..4 {
        print!("{:>8}", MATS[k]);
        for v in conf[k].iter() {
            print!("{:>8}", v);
        }
        println!();
        correct += conf[k][k];
        total += conf[k].iter().sum::<usize>();
    }
    println!(
        "留出准确率 {correct}/{total} = {:.1}%",
        100.0 * correct as f32 / total.max(1) as f32
    );
    if let Some(p) = arg(argv, "--out") {
        let mut s = format!("lambda {lambda}\n");
        for (j, f) in LOOK_FEATURES.iter().enumerate() {
            s.push_str(&format!("mu {f} {:.6}\n", mu[j]));
            s.push_str(&format!("sd {f} {:.6}\n", sd[j]));
        }
        for (k, w) in head.iter().enumerate() {
            for (j, v) in w.iter().enumerate() {
                s.push_str(&format!("w {k} {j} {v:.6}\n"));
            }
        }
        std::fs::write(&p, s).unwrap_or_else(|e| fail(&format!("{p}: {e}")));
        println!("模型写出 {p}");
    }
}

fn tex_quilt_cmd(argv: &[String]) {
    use trit_parts::tex::quilt;
    let Some(p) = argv.get(1).filter(|a| !a.starts_with("--")) else {
        fail(
            "用法: parts-3d tex-quilt <源.png> [--out out.ppm] [--size 128] [--patch 12] [--overlap 6] [--seed 1]",
        );
    };
    let (sw, sh, rgb) =
        trit_parts::data::read_rgb(std::path::Path::new(p)).unwrap_or_else(|e| fail(&e));
    let n: usize = arg(argv, "--size")
        .and_then(|v| v.parse().ok())
        .unwrap_or(128);
    let patch: usize = arg(argv, "--patch")
        .and_then(|v| v.parse().ok())
        .unwrap_or(12);
    // 实测（§6.47）：重叠越大越糊、环绕接缝越差（ov=12 时 border 2.0）⇒ 默认 patch/4
    let ov: usize = arg(argv, "--overlap")
        .and_then(|v| v.parse().ok())
        .unwrap_or(patch / 4);
    let seed: u64 = arg(argv, "--seed")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let out = quilt(&rgb, sw, sh, n, n, patch, ov, seed);
    let l = trit_parts::look::analyze(&out, n, n);
    println!("源 {sw}x{sh} ⇒ {n}x{n}，patch {patch} 重叠 {ov} seed {seed}");
    println!(
        "stroke {:.2} coherence {:.3} hf1 {:.3} contrast {:.3}",
        l.stroke, l.coherence, l.hf1, l.contrast
    );
    if let Some(outp) = arg(argv, "--out") {
        let mut bytes = format!("P6\n{n} {n}\n255\n").into_bytes();
        for v in &out {
            bytes.push((v.clamp(0.0, 1.0) * 255.0) as u8);
        }
        std::fs::write(&outp, bytes).unwrap_or_else(|e| fail(&format!("{outp}: {e}")));
        println!("写出 {outp}");
    }
}

/// 从 manifest 里抽 `pass_index → material`（手写扫描：manifest 由本仓渲染器产出，字段顺序固定）。
fn materials_by_pass(manifest: &str) -> Vec<Option<String>> {
    let mut out: Vec<Option<String>> = Vec::new();
    let mut rest = manifest;
    while let Some(i) = rest.find("\"pass_index\":") {
        let after = &rest[i + 13..];
        rest = after;
        let num: String = after
            .trim_start()
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        let Ok(idx) = num.parse::<usize>() else {
            continue;
        };
        // 在同一对象内找 material（下一个 pass_index 之前）
        let seg_end = after.find("\"pass_index\":").unwrap_or(after.len());
        let window = &after[..seg_end];
        let mat = window.find("\"material\":").and_then(|j| {
            let tail = &window[j + 11..];
            let q = tail.find('"')? + 1;
            let name: String = tail[q..].chars().take_while(|c| *c != '"').collect();
            if name.is_empty() { None } else { Some(name) }
        });
        if idx >= out.len() {
            out.resize(idx + 1, None);
        }
        out[idx] = mat;
    }
    out
}

/// 从命令行取风格参数（预设优先；都没有/默认值则返回 None = 不变）。
fn style_from_argv(argv: &[String]) -> Option<trit_parts::tex::Style> {
    use trit_parts::tex::{Preset, Style};
    if let Some(name) = arg(argv, "--preset") {
        let p = Preset::by_name(&name)
            .unwrap_or_else(|| fail(&format!("未知预设 {name}（可选 cel / oil / epic）")));
        let t = arg(argv, "--strength")
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(1.0);
        return Some(p.style(t));
    }
    let st = Style {
        contrast: arg(argv, "--contrast")
            .and_then(|v| v.parse().ok())
            .unwrap_or(1.0),
        levels: arg(argv, "--levels")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0f32)
            .max(0.0) as u32,
        warmth: arg(argv, "--warmth")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0),
        sat: arg(argv, "--sat")
            .and_then(|v| v.parse().ok())
            .unwrap_or(1.0),
        stroke: arg(argv, "--stroke")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0),
    };
    if st.is_identity() { None } else { Some(st) }
}

/// `tex-stylize <in.png> --contrast 1.2 --levels 8 --warmth 0.1 --sat 1.5 --stroke 2 --out o.png`：
/// S6.5 的**显式**风格旋钮（不训练）；打印变换前后 trit-look 的对应因子，便于验收单调性。
fn tex_stylize_cmd(argv: &[String]) {
    use trit_parts::tex::stylize;
    let Some(p) = argv.get(1).filter(|a| !a.starts_with("--")) else {
        fail(
            "用法: parts-3d tex-stylize <in.png> [--contrast 1.2] [--levels 8] [--warmth 0.1] [--sat 1.5] [--stroke 2] --out o.png",
        );
    };
    let (w, h, px) =
        trit_parts::data::read_rgb(std::path::Path::new(p)).unwrap_or_else(|e| fail(&e));
    let st = style_from_argv(argv).unwrap_or_default();
    let out = stylize(&px, w, h, &st);
    let a = trit_parts::look::analyze(&px, w, h);
    let b = trit_parts::look::analyze(&out, w, h);
    println!("{:<10} {:>9} -> {:>9}", "factor", "before", "after");
    for (name, x, y) in [
        ("stroke", a.stroke, b.stroke),
        ("levels", a.levels, b.levels),
        ("strong", a.strong, b.strong),
        ("contrast", a.contrast, b.contrast),
        ("warmth", a.warmth, b.warmth),
        ("sat", a.sat, b.sat),
    ] {
        println!("{name:<10} {x:>9.3} -> {y:>9.3}");
    }
    if let Some(o) = arg(argv, "--out") {
        trit_parts::data::write_rgb(std::path::Path::new(&o), w, h, &out)
            .unwrap_or_else(|e| fail(&e));
        println!("写出 {o}");
    }
}

fn tex_gen_cmd(argv: &[String]) {
    use trit_parts::tex::{seam_ratio, tile};

    let iters: usize = arg(argv, "--iters")
        .and_then(|v| v.parse().ok())
        .unwrap_or(200000);
    let n: usize = arg(argv, "--size")
        .and_then(|v| v.parse().ok())
        .unwrap_or(64);
    let steps: usize = arg(argv, "--steps-t")
        .and_then(|v| v.parse().ok())
        .unwrap_or(50);
    let seed: u64 = arg(argv, "--seed")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let eval_seed: u64 = arg(argv, "--eval-seed")
        .and_then(|v| v.parse().ok())
        .unwrap_or(2);
    // 0.003 是实测能收敛的档；0.01 在真实材质上会从 loss 0.62 涨到 6.48（GENERAL-PLAN §6.21）
    let lr: f32 = arg(argv, "--lr")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.003);
    let hid: usize = arg(argv, "--hid")
        .and_then(|v| v.parse().ok())
        .unwrap_or(32);
    let outdir = arg(argv, "--out").map(PathBuf::from);
    let data_dir = arg(argv, "--data").map(PathBuf::from);
    let patch: usize = arg(argv, "--patch")
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);
    // --normalize：按图 z-score 到单位方差（扩散噪声表按单位方差设计；也顺带得到显式亮度/对比控制）
    let normalize = argv.iter().any(|a| a == "--normalize");

    let kinds = ["wood", "marble", "rust", "grid"];
    let gray = |kind: &str, sd: u64| -> Vec<f32> {
        tile(kind, n, sd)
            .iter()
            .map(|p| (0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32) / 255.0)
            .collect()
    };
    let sched = Schedule::cosine(steps);
    let abar = sched.abar();
    // 数据源：--data <dir> 用真实材质（每张图一个样本，无条件）；否则程序化材质
    let gray_img = |f: &PathBuf| -> Vec<f32> {
        let (w0, h0, pix) = trit_parts::data::read_rgb(f).unwrap_or_else(|e| fail(&e));
        let rgb = fit_rgb(w0, h0, &pix, n);
        rgb.as_chunks::<3>()
            .0
            .iter()
            .map(|c| 0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2])
            .collect()
    };
    // 评测图必须钉死：`--holdout-names` 按文件名前缀匹配（扩数据集时排序会变，`take(2)` 会漂）
    let hold_names: Vec<String> = arg(argv, "--holdout-names")
        .map(|v| {
            v.split(',')
                .map(|x| x.trim().to_string())
                .filter(|x| !x.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let mut img_files: Vec<PathBuf> = Vec::new();
    let images: Vec<Vec<f32>> = match &data_dir {
        Some(dir) => {
            let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
                .unwrap_or_else(|e| fail(&format!("读不了 {}: {e}", dir.display())))
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "png"))
                .collect();
            files.sort();
            if files.len() < 3 {
                fail("--data 至少 3 张 PNG");
            }
            files.retain(|f| {
                let stem = f
                    .file_stem()
                    .map(|x| x.to_string_lossy().into_owned())
                    .unwrap_or_default();
                !hold_names.iter().any(|h| stem.starts_with(h.as_str()))
            });
            if !hold_names.is_empty() && files.len() < 3 {
                fail("--holdout-names 之后训练集不足 3 张");
            }
            img_files = files.clone();
            files.iter().map(gray_img).collect()
        }
        None => kinds.iter().map(|k| gray(k, seed)).collect(),
    };
    // 评测集：真实材质时用留出名单里的那几张
    let eval_real: Vec<(String, Vec<f32>)> = match &data_dir {
        Some(dir) if !hold_names.is_empty() => {
            let mut v = Vec::new();
            for f in std::fs::read_dir(dir)
                .unwrap_or_else(|e| fail(&format!("读不了 {}: {e}", dir.display())))
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "png"))
            {
                let stem = f
                    .file_stem()
                    .map(|x| x.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if hold_names.iter().any(|h| stem.starts_with(h.as_str())) {
                    v.push((stem.clone(), f.clone()));
                }
            }
            v.sort_by(|a, b| a.0.cmp(&b.0));
            v.into_iter()
                .map(|(name, f)| (name, gray_img(&f)))
                .collect()
        }
        _ => Vec::new(),
    };
    // 训练集均值是有用的校准读数（采样塌成常数时先看它是不是偏了）
    let data_mean = images
        .iter()
        .map(|g| g.iter().sum::<f32>() / g.len() as f32)
        .sum::<f32>()
        / images.len() as f32;
    println!("训练图 {} 张，均值 {data_mean:.3}", images.len());
    // S6.3：材质类别（--labels <tsv: assetId\tclass>）→ 每张图的类索引
    let mut film_k: usize = arg(argv, "--film")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut img_class: Vec<Option<usize>> = vec![None; images.len()];
    if film_k > 0 {
        let labels_path = arg(argv, "--labels")
            .unwrap_or_else(|| fail("--film 需要 --labels <tsv>（assetId\\tclass）"));
        let txt = std::fs::read_to_string(&labels_path)
            .unwrap_or_else(|e| fail(&format!("读不了 {labels_path}: {e}")));
        let mut names: Vec<String> = Vec::new();
        let mut ids: Vec<String> = Vec::new();
        for line in txt.lines() {
            let mut it = line.split('\t');
            let (Some(aid), Some(cls)) = (it.next(), it.next()) else {
                continue;
            };
            let name = cls.trim();
            let idx = match names.iter().position(|n| n == name) {
                Some(i) => i,
                None => {
                    names.push(name.to_string());
                    names.len() - 1
                }
            };
            ids.push(format!("{aid}\t{idx}"));
        }
        if names.len() != film_k {
            fail(&format!(
                "--film {film_k} 与标签里的类数 {} 不一致：{:?}",
                names.len(),
                names
            ));
        }
        for (i, f) in img_files.iter().enumerate() {
            let stem = f
                .file_stem()
                .map(|x| x.to_string_lossy().into_owned())
                .unwrap_or_default();
            for row in &ids {
                let (aid, idx) = row.split_once('\t').unwrap();
                if stem.starts_with(aid) {
                    img_class[i] = idx.parse().ok();
                    break;
                }
            }
        }
        let hit = img_class.iter().filter(|c| c.is_some()).count();
        println!(
            "类别条件：{film_k} 类，命中 {hit}/{} 张；类名 {:?}",
            images.len(),
            names
        );
    }

    // --classes a,b：只保留这些类并重映射到 0..n（S6.3 的小规模对照）
    let sel: Vec<usize> = arg(argv, "--classes")
        .map(|v| v.split(',').filter_map(|x| x.trim().parse().ok()).collect())
        .unwrap_or_default();
    let images: Vec<Vec<f32>> = if sel.len() >= 2 && film_k > 0 {
        let keep: Vec<usize> = (0..images.len())
            .filter(|&i| img_class[i].is_some_and(|c| sel.contains(&c)))
            .collect();
        if keep.len() < 40 {
            fail(&format!("--classes {sel:?} 只有 {} 张，太少", keep.len()));
        }
        img_class = keep
            .iter()
            .map(|&i| img_class[i].map(|c| sel.iter().position(|&s| s == c).unwrap()))
            .collect();
        film_k = sel.len();
        println!("只训类 {sel:?}：{} 张（重映射 0..{}）", keep.len(), film_k);
        keep.iter().map(|&i| images[i].clone()).collect()
    } else {
        images
    };

    // 归一化：每张图减去自身均值、除以自身标准差（保留反归一化参数）
    let img_norm: Vec<(f32, f32)> = images
        .iter()
        .map(|g| {
            let m = g.iter().sum::<f32>() / g.len() as f32;
            let v = g.iter().map(|x| (x - m) * (x - m)).sum::<f32>() / g.len() as f32;
            (m, v.sqrt().max(1e-4))
        })
        .collect();
    let images: Vec<Vec<f32>> = if normalize {
        images
            .iter()
            .zip(&img_norm)
            .map(|(g, (m, s))| g.iter().map(|x| (x - m) / s).collect())
            .collect()
    } else {
        images
    };
    let conds = if data_dir.is_some() { 0 } else { kinds.len() };
    // 去噪器：默认 3×3/7×7 补丁 MLP；`--conv 8,8` 换成三值卷积栈（输入 = [x_t, t/T] 两张图）
    let conv_hidden: Vec<usize> = arg(argv, "--conv")
        .map(|v| v.split(',').filter_map(|x| x.trim().parse().ok()).collect())
        .unwrap_or_default();
    let use_bn = argv.iter().any(|a| a == "--bn");
    let use_adam = argv.iter().any(|a| a == "--adam");
    // ε-预测：目标是噪声而不是 x0；线性输出 + 平方误差（x0-预测会把细纹理平均掉）
    let v_mode = argv.iter().any(|a| a == "--v");
    // --cond-stats：把「这张图的均值 + 对比(p95−p5)」作为两路常数条件图，治外观单模
    let cond_stats = argv.iter().any(|a| a == "--cond-stats");
    // --normalize：按图 z-score 到单位方差（扩散噪声表是按单位方差设计的；也顺带得到显式的亮度/对比控制）
    let eps_mode = argv.iter().any(|a| a == "--eps") && !v_mode;
    let linear_mode = eps_mode || v_mode;
    let batch: usize = arg(argv, "--batch")
        .and_then(|v| v.parse().ok())
        .unwrap_or(4);
    let save_path = arg(argv, "--save").map(PathBuf::from);
    let load_path = arg(argv, "--load").map(PathBuf::from);
    // 没有 --conv 时走的是 MLP 分支，那里根本不看 --save/--load：
    // 曾经 `--load` 被静默忽略，采到的是未训练 MLP 的常数图（2026-11-01 实测三份不同 checkpoint 输出逐位相同）。
    if conv_hidden.is_empty() && (load_path.is_some() || save_path.is_some()) {
        fail("--load/--save 只对卷积去噪器生效：请同时给 --conv（隐藏宽度，如 --conv 8,8）");
    }
    let mut rng: u64 = 0x9E37_79B9_7F4A_7C15 ^ seed;
    let mut next_u = || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        (rng >> 11) as f32 / (1u64 << 53) as f32
    };
    let mut loss = 0.0f64;
    let den: Den = if conv_hidden.is_empty() {
        let mut d = PatchMlp::new(patch, conds, hid, seed);
        for step in 0..iters {
            let ki = (step * 7919) % images.len();
            let g = images[ki].clone();
            loss += d.train_shard(&g, &abar, None, n, n, lr, 64, &mut rng) as f64;
            if (step + 1) % 2000 == 0 {
                println!("  step {} loss {:.4}", step + 1, loss / 2000.0);
                loss = 0.0;
            }
        }
        Den::M(d)
    } else {
        let mut net = match &load_path {
            Some(p) => Net::load(p).unwrap_or_else(|e| fail(&e)),
            None => Net::new(2 + 2 * usize::from(cond_stats), &conv_hidden, seed),
        };
        if let Some(d) = arg(argv, "--dil").and_then(|v| v.parse::<usize>().ok()) {
            let file_dil = net.layers.first().map(|l| l.dil).unwrap_or(1);
            if load_path.is_some() && d.max(1) != file_dil {
                fail(&format!(
                    "--dil {d} 与 checkpoint 里的膨胀率 {file_dil} 不一致（改它等于换模型）"
                ));
            }
            net.set_dilation(d);
        }
        net.set_bn(use_bn && load_path.is_none());
        // 可平铺纹理用循环填充：零填充会把边界抹平（接缝比 0.60）
        if argv.iter().any(|a| a == "--circ") {
            net.set_circular(true);
        }
        // 高通匹配权重（对抗 MSE 的条件均值化；0 = 关）。
        // 只有显式给了才覆盖：否则加载 TPC4+ checkpoint 时会把文件里的 highpass 静默抹成 0。
        if let Some(v) = arg(argv, "--hf-weight").and_then(|v| v.parse().ok()) {
            net.highpass = v;
        }
        if linear_mode && load_path.is_none() {
            net.linear_out = true;
        }
        if film_k > 0 && load_path.is_none() {
            net.set_film(film_k);
            net.film_lr_mul = arg(argv, "--film-lr")
                .and_then(|v| v.parse().ok())
                .unwrap_or(1.0f32);
            println!("FiLM lr 倍数 {}", net.film_lr_mul);
            // coherence 匹配（归一化矩，直击各向异性）
            if let (Some(c), Some(cs), Some(ts), Some(lam)) = (
                arg(argv, "--coh-class").and_then(|v| v.parse::<usize>().ok()),
                arg(argv, "--coh-target").and_then(|v| v.parse::<f32>().ok()),
                arg(argv, "--coh-energy").and_then(|v| v.parse::<f32>().ok()),
                arg(argv, "--coh-loss").and_then(|v| v.parse::<f32>().ok()),
            ) {
                let w_t = arg(argv, "--coh-wt")
                    .and_then(|v| v.parse::<f32>().ok())
                    .unwrap_or(0.1f32);
                net.coh = Some((c, cs, ts, lam, w_t));
                net.abar = abar.clone();
                println!("coherence 匹配：类 {c}，c*={cs}，T*={ts}，λ={lam}，能量权 {w_t}");
            }
            // 结构张量矩匹配（S6.3 方向性结构）
            if let (Some(c), Some(t), Some(lam)) = (
                arg(argv, "--stm-class").and_then(|v| v.parse::<usize>().ok()),
                arg(argv, "--stm-target"),
                arg(argv, "--stm-loss").and_then(|v| v.parse::<f32>().ok()),
            ) {
                let parts: Vec<f32> = t.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                if parts.len() == 3 {
                    net.stm = Some((c, [parts[0], parts[1], parts[2]], lam));
                    net.abar = abar.clone();
                    println!("结构张量匹配：类 {c}，目标 {:?}，λ={lam}", parts);
                }
            }
            // 方向性损失：只对指定类（如木纹）抑制垂直方向梯度
            if let (Some(c), Some(deg), Some(lam)) = (
                arg(argv, "--dir-class").and_then(|v| v.parse::<usize>().ok()),
                arg(argv, "--dir-angle").and_then(|v| v.parse::<f32>().ok()),
                arg(argv, "--dir-loss").and_then(|v| v.parse::<f32>().ok()),
            ) {
                net.dir = Some((c, deg.to_radians(), lam));
                net.abar = abar.clone();
                println!("方向性损失：类 {c}，目标梯度角 {deg}°，λ={lam}");
            }
            // 每类一个输出头（共享主干）：把「逐类上界」的机制塞回单模型
            if argv.iter().any(|a| a == "--heads") {
                net.set_heads(film_k, seed);
                println!("输出头：{film_k} 个（共享主干）");
            }
            // 辅助分类损失：逼输出携带类别信息（FiLM 单独用会被逐点回归忽略）
            net.aux_w = arg(argv, "--aux")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.0f32);
            if net.aux_w > 0.0 {
                net.set_aux(film_k);
                println!("辅助分类损失 λ={}（{} 类）", net.aux_w, film_k);
            }
        }
        let tmax = sched.steps().max(1);
        // 每张图的 (均值, 对比) —— 条件通道的取值
        let img_stats: Vec<(f32, f32)> = images
            .iter()
            .map(|g| {
                let mean = g.iter().sum::<f32>() / g.len() as f32;
                let mut v = g.clone();
                v.sort_by(|a, b| a.partial_cmp(b).unwrap());
                let p = |q: f32| v[((v.len() - 1) as f32 * q).round() as usize];
                (mean, p(0.95) - p(0.05))
            })
            .collect();
        for step in 0..iters {
            let mut items: Vec<trit_parts::tconv::Sample> = Vec::with_capacity(batch);
            for _ in 0..batch {
                let ki = (next_u() * images.len() as f32) as usize % images.len();
                let t = 1 + (next_u() * tmax as f32) as usize % tmax;
                let noise: Vec<f32> = (0..n * n)
                    .map(|_| {
                        let u1 = next_u().max(1e-6);
                        let u2 = next_u();
                        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos()
                    })
                    .collect();
                let x0 = &images[ki];
                let xt = trit_parts::diffuse::add_noise(x0, &noise, abar[t]);
                let tf = t as f32 / tmax as f32;
                let mut maps = vec![xt, vec![tf; n * n]];
                if cond_stats {
                    let (m, c) = img_stats[ki];
                    maps.push(vec![m; n * n]);
                    maps.push(vec![c; n * n]);
                }
                // 10% 条件丢弃（学会空类，避免只能靠类别提示）
                let cls = if film_k > 0 && next_u() < 0.1 {
                    None
                } else {
                    img_class[ki]
                };
                let target = if eps_mode {
                    noise
                } else if v_mode {
                    // v = √ᾱ·ε − √(1−ᾱ)·x0
                    let (sa, se) = (abar[t].sqrt(), (1.0 - abar[t]).sqrt());
                    noise
                        .iter()
                        .zip(x0.iter())
                        .map(|(e, c)| sa * e - se * c)
                        .collect()
                } else {
                    x0.clone()
                };
                items.push((maps, target, cls));
            }
            loss += net.train_batch_xc(&items, n, n, lr, use_adam) as f64;
            if (step + 1) % 2000 == 0 {
                println!(
                    "  step {} loss {:.4} | aux CE {:.3} acc {:.2}",
                    step + 1,
                    loss / 2000.0,
                    net.aux_ce,
                    net.aux_acc
                );
                loss = 0.0;
            }
            // 断点：每 1000 步存一次（本机常被关机，别让整轮白跑）
            if let Some(p) = &save_path
                && (step + 1) % 1000 == 0
            {
                net.save(p).unwrap_or_else(|e| fail(&e));
                println!("  ckpt {} @ step {}", p.display(), step + 1);
            }
        }
        // --diag：线性输出（ε/v）时，判预测器对输入 x_t 的依赖强度。
        // 只换噪声、同 t、同 x0：若 v̂/ε̂ 几乎不变，说明输入被 STE 分流掉了（种子不变性的根因）。
        if argv.iter().any(|a| a == "--diag") && linear_mode {
            let mut r = 0x5EED_1234u64;
            let mut g = |k: usize| -> Vec<f32> {
                (0..k)
                    .map(|_| {
                        r ^= r << 13;
                        r ^= r >> 7;
                        r ^= r << 17;
                        let u1 = ((r >> 11) as f32 / (1u64 << 53) as f32).max(1e-6);
                        r ^= r << 13;
                        r ^= r >> 7;
                        r ^= r << 17;
                        let u2 = (r >> 11) as f32 / (1u64 << 53) as f32;
                        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos()
                    })
                    .collect()
            };
            let corr = |a: &[f32], b: &[f32]| -> f32 {
                let n = a.len() as f32;
                let (ma, mb) = (a.iter().sum::<f32>() / n, b.iter().sum::<f32>() / n);
                let (mut sab, mut sa, mut sb) = (0.0f32, 0.0f32, 0.0f32);
                for (x, y) in a.iter().zip(b) {
                    let (dx, dy) = (x - ma, y - mb);
                    sab += dx * dy;
                    sa += dx * dx;
                    sb += dy * dy;
                }
                sab / (sa * sb).sqrt().max(1e-9)
            };
            let rms = |v: &[f32]| -> f32 {
                (v.iter().map(|x| x * x).sum::<f32>() / v.len() as f32).sqrt()
            };
            let x0 = &images[0];
            for tf in [0.2f32, 0.5, 0.8, 1.0] {
                let t = ((tf * tmax as f32).round() as usize).clamp(1, tmax);
                let e1 = g(n * n);
                let e2 = g(n * n);
                let xt1 = trit_parts::diffuse::add_noise(x0, &e1, abar[t]);
                let xt2 = trit_parts::diffuse::add_noise(x0, &e2, abar[t]);
                let h1 = net.forward_x(&[xt1.clone(), vec![tf; n * n]], n, n);
                let h2 = net.forward_x(&[xt2, vec![tf; n * n]], n, n);
                // 目标：ε 或 v
                let (sa, se) = (abar[t].sqrt(), (1.0 - abar[t]).sqrt());
                let target: Vec<f32> = if v_mode {
                    e1.iter()
                        .zip(x0.iter())
                        .map(|(e, c)| sa * e - se * c)
                        .collect()
                } else {
                    e1.clone()
                };
                let d: Vec<f32> = h1.iter().zip(&h2).map(|(a, b)| a - b).collect();
                let dt: Vec<f32> = h1.iter().zip(&target).map(|(a, b)| a - b).collect();
                println!(
                    "  依赖 t={tf:.1}: corr(ĥ,目标)={:.3} 残差 {:.3} | 换噪声后相对变化 **{:.1}%** | corr(ĥ,x_t)={:.3}",
                    corr(&h1, &target),
                    rms(&dt),
                    100.0 * rms(&d) / rms(&h1).max(1e-6),
                    corr(&h1, &xt1)
                );
            }
        }
        if let Some(p) = &save_path {
            net.save(p).unwrap_or_else(|e| fail(&e));
            println!("权重已存 {}", p.display());
        }
        if v_mode {
            let extra = if cond_stats {
                let want_mean: f32 = arg(argv, "--want-mean")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_else(|| {
                        img_stats.iter().map(|s| s.0).sum::<f32>() / img_stats.len() as f32
                    });
                let want_contrast: f32 = arg(argv, "--want-contrast")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_else(|| {
                        let mut c: Vec<f32> = img_stats.iter().map(|s| s.1).collect();
                        c.sort_by(|a, b| a.partial_cmp(b).unwrap());
                        c[c.len() / 2]
                    });
                println!("条件: 目标均值 {want_mean:.3}、目标对比 {want_contrast:.3}");
                vec![want_mean, want_contrast]
            } else {
                Vec::new()
            };
            let want_class: Option<usize> = arg(argv, "--want-class")
                .and_then(|v| v.parse().ok())
                .filter(|&k| k < film_k);
            if film_k > 0 {
                println!(
                    "采样类别：{}",
                    want_class
                        .map(|k| k.to_string())
                        .unwrap_or_else(|| "空类".into())
                );
            }
            Den::V(trit_parts::tconv::VDen {
                net,
                abar: abar.clone(),
                extra,
                clamp: !normalize,
                class: want_class,
            })
        } else if eps_mode {
            Den::E(trit_parts::tconv::EpsDen {
                net,
                abar: abar.clone(),
                eps_scale: arg(argv, "--eps-scale")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(1.0f32),
            })
        } else {
            Den::C(net)
        }
    };

    // 诊断：在 t/T = 0.2/0.5/0.8 上加噪，看 x0 预测的 MAE（应随时间步增大而变大）
    {
        let g = match (&data_dir, eval_real.first()) {
            (Some(_), Some((_, im))) => im.clone(),
            _ => gray(kinds[0], eval_seed),
        };
        let mut rng2: u64 = 999;
        let mut next = || {
            rng2 ^= rng2 << 13;
            rng2 ^= rng2 >> 7;
            rng2 ^= rng2 << 17;
            (rng2 >> 11) as f32 / (1u64 << 53) as f32
        };
        for &tf in &[0.2f32, 0.5, 0.8] {
            let t = ((tf * (abar.len() - 1) as f32) as usize).min(abar.len() - 1);
            let eps: Vec<f32> = (0..n * n)
                .map(|_| {
                    let u1 = next().max(1e-6);
                    let u2 = next();
                    (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos()
                })
                .collect();
            let xt = trit_parts::diffuse::add_noise(&g, &eps, abar[t]);
            let pred = den.predict_x0(&xt, tf, if conds == 0 { None } else { Some(0) }, n, n);
            let mae = pred.iter().zip(&g).map(|(a, b)| (a - b).abs()).sum::<f32>() / (n * n) as f32;
            let pmid = pred.iter().sum::<f32>() / pred.len() as f32;
            println!(
                "  诊断 t={tf:.1}: 预测 MAE {mae:.3}，预测均值 {pmid:.3}（真值均值 {:.3}）",
                g.iter().sum::<f32>() / g.len() as f32
            );
        }
    }

    let stat = |v: &[f32]| -> (f32, f32) {
        let m = v.iter().sum::<f32>() / v.len() as f32;
        let var = v.iter().map(|x| (x - m) * (x - m)).sum::<f32>() / v.len() as f32;
        (m, var.sqrt())
    };
    let patch_dist = |img: &[f32], data: &[f32]| -> f32 {
        // 每 8 像素取一个 patch² 补丁，报生成补丁到真实补丁的最小 L2 均值
        let stride = 8usize;
        let half = patch / 2;
        let grab = |src: &[f32], x: usize, y: usize| -> Vec<f32> {
            let mut p = Vec::with_capacity(patch * patch);
            for dy in 0..patch {
                for dx in 0..patch {
                    p.push(src[(y + dy - half) * n + x + dx - half]);
                }
            }
            p
        };
        let mut refs: Vec<Vec<f32>> = Vec::new();
        for y in (half..n - half).step_by(stride) {
            for x in (half..n - half).step_by(stride) {
                refs.push(grab(data, x, y));
            }
        }
        let mut sum = 0.0f32;
        let mut cnt = 0usize;
        for y in (half..n - half).step_by(stride) {
            for x in (half..n - half).step_by(stride) {
                let p = grab(img, x, y);
                let mut best = f32::INFINITY;
                for r in &refs {
                    let d2: f32 = p.iter().zip(r).map(|(a, b)| (a - b) * (a - b)).sum();
                    best = best.min(d2);
                }
                sum += best.sqrt();
                cnt += 1;
            }
        }
        sum / cnt.max(1) as f32
    };

    println!("材质     生成(均值±std)      真实(均值±std)     接缝比  最近补丁距（patch {patch}）");
    let eval_items: Vec<(String, Vec<f32>)> = match &data_dir {
        Some(_) if !eval_real.is_empty() => eval_real.clone(),
        Some(_) => images
            .iter()
            .take(2)
            .enumerate()
            .map(|(i, g)| (format!("data{i}"), g.clone()))
            .collect(),
        None => kinds
            .iter()
            .enumerate()
            .map(|(ki, k)| ((*k).to_string(), gray(k, eval_seed + ki as u64)))
            .collect(),
    };
    for (ki, (k, data)) in eval_items.iter().enumerate() {
        let data = data.clone();
        let (dm, ds) = stat(&data);
        let sample = den.sample(
            &sched,
            n,
            n,
            eval_seed * 131 + ki as u64 + 7,
            if data_dir.is_some() { None } else { Some(ki) },
            argv.iter().any(|a| a == "--eps-space"),
            !normalize,
        );
        let sample: Vec<f32> = if normalize {
            let want_mean: f32 = arg(argv, "--want-mean")
                .and_then(|v| v.parse().ok())
                .unwrap_or_else(|| {
                    let mut m: Vec<f32> = img_norm.iter().map(|s| s.0).collect();
                    m.sort_by(|a, b| a.partial_cmp(b).unwrap());
                    m[m.len() / 2]
                });
            let want_std: f32 = arg(argv, "--want-std")
                .and_then(|v| v.parse().ok())
                .unwrap_or_else(|| {
                    let mut s: Vec<f32> = img_norm.iter().map(|s| s.1).collect();
                    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
                    s[s.len() / 2]
                });
            println!("反归一化: 目标均值 {want_mean:.3}、目标标准差 {want_std:.3}");
            sample.iter().map(|x| x * want_std + want_mean).collect()
        } else {
            sample
        };
        let (gm, gs) = stat(&sample);
        let sr = seam_ratio(
            &sample
                .iter()
                .map(|v| [(v * 255.0) as u8; 3])
                .collect::<Vec<_>>(),
            n,
        );
        let pd = patch_dist(&sample, &data);
        println!("{k:<8} {gm:.3}±{gs:.3}      {dm:.3}±{ds:.3}      {sr:.2}    {pd:.3}");
        let _ = &data;
        if let Some(dir) = &outdir {
            std::fs::create_dir_all(dir).ok();
            let mut bytes = format!(
                "P6
{n} {n}
255
"
            )
            .into_bytes();
            for v in &sample {
                let c = (v.clamp(0.0, 1.0) * 255.0) as u8;
                bytes.extend_from_slice(&[c, c, c]);
            }
            std::fs::write(dir.join(format!("gen_{k}.ppm")), bytes).ok();
        }
    }
    println!("（接缝比 ≈1 表示边界与内部同量级；最近补丁距太小 = 在抄训练集）");
}

/// `look <图或目录>… [--out csv]`：量「观感」——亮度/对比/饱和/色相熵/细节/平涂/雾感/发光/UI 边条。
/// 启发式描述量，不训练；同一输入两次跑逐位一致（见 `look` 模块单测）。
fn look_cmd(argv: &[String]) {
    use std::io::Write as _;
    use trit_parts::look;

    let csv = arg(argv, "--out").map(PathBuf::from);
    let mut inputs: Vec<PathBuf> = Vec::new();
    let mut skip_next = false;
    for a in argv.iter().skip(1) {
        if skip_next {
            skip_next = false;
            continue;
        }
        if a == "--out" {
            skip_next = true;
            continue;
        }
        if a.starts_with("--") {
            continue;
        }
        let p = PathBuf::from(a);
        if p.is_dir() {
            let mut v: Vec<PathBuf> = std::fs::read_dir(&p)
                .unwrap_or_else(|e| fail(&format!("读不了 {}: {e}", p.display())))
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|q| q.extension().is_some_and(|x| x == "png"))
                .collect();
            v.sort();
            inputs.extend(v);
        } else {
            inputs.push(p);
        }
    }
    if inputs.is_empty() {
        fail("用法: parts-3d look <图.png 或目录>… [--out features.csv]");
    }

    // 用 writeln! 吞掉 BrokenPipe：`parts-3d look … | head` 时不该 panic
    let mut out_h = std::io::stdout().lock();
    let _ = writeln!(
        out_h,
        "{:<22} {:>5} {:>5} {:>5} {:>5} {:>5} {:>5} {:>5} {:>5} {:>5} {:>5} {:>5} {:>6} {:>5} {:>5} {:>5} {:>6} {:>6} {:>6} {:>6}  启发式",
        "样本",
        "亮度",
        "对比",
        "饱和",
        "色相熵",
        "细纹",
        "硬边",
        "平坦",
        "级数",
        "雾感",
        "发光",
        "边条",
        "冷暖",
        "主光°",
        "光比",
        "高光",
        "hf1",
        "hf2",
        "hf4",
        "边界比"
    );
    let mut rows: Vec<(String, look::Look)> = Vec::new();
    for f in &inputs {
        let name = f
            .file_stem()
            .map(|x| x.to_string_lossy().into_owned())
            .unwrap_or_default();
        let (w, h, rgb) = trit_parts::data::read_rgb(f).unwrap_or_else(|e| fail(&e));
        let l = look::analyze(&rgb, w, h);
        let _ = writeln!(
            out_h,
            "{:<22} {:>5.2} {:>5.2} {:>5.2} {:>5.2} {:>5.3} {:>5.3} {:>5.2} {:>5.0} {:>5.2} {:>5.3} {:>5.2} {:>6.2} {:>5.0} {:>5.2} {:>5.3} {:>6.3} {:>6.3} {:>6.3} {:>6.3}  {}",
            name,
            l.luma,
            l.contrast,
            l.sat,
            l.hue_entropy,
            l.fine,
            l.strong,
            l.flat,
            l.levels,
            l.haze,
            l.emissive,
            l.bars,
            l.warmth,
            l.key_dir,
            l.key_fill_dwarm,
            l.highlights,
            l.hf1,
            l.hf2,
            l.hf4,
            l.border,
            look::guess(&l)
        );
        rows.push((name, l));
    }

    if let Some(p) = csv {
        let mut out = String::from("name,luma,contrast,sat,hue_entropy,edge,fine,strong,flat,levels,haze,emissive,bars,warmth,key_dir,key_fill_dwarm,highlights,hf1,hf2,hf4,border
");
        for (n, l) in &rows {
            out.push_str(&format!(
                "{n},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.2},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4}
",
                l.luma, l.contrast, l.sat, l.hue_entropy, l.edge, l.fine, l.strong, l.flat, l.levels,
                l.haze, l.emissive, l.bars, l.warmth, l.key_dir, l.key_fill_dwarm, l.highlights, l.hf1, l.hf2, l.hf4, l.border
            ));
        }
        std::fs::write(&p, out).unwrap_or_else(|e| fail(&format!("{}: {e}", p.display())));
        println!("特征已写 {}", p.display());
    }
}

/// 取 n×n 训练/评测图：源图够大时**按原分辨率中心裁剪**，不做重采样。
/// 重采样会把细纹理（如沥青颗粒）直接抹平，模型就永远学不到 1 像素级的细节。
fn fit_rgb(w0: usize, h0: usize, pix: &[f32], n: usize) -> Vec<f32> {
    if w0 == n && h0 == n {
        return pix.to_vec();
    }
    if w0 >= n && h0 >= n {
        let (x0, y0) = ((w0 - n) / 2, (h0 - n) / 2);
        let mut out = Vec::with_capacity(n * n * 3);
        for y in 0..n {
            let base = ((y0 + y) * w0 + x0) * 3;
            out.extend_from_slice(&pix[base..base + n * 3]);
        }
        return out;
    }
    trit_parts::data::resize_rgb(w0, h0, pix, n)
}

/// `style <A…> [--vs <B…>] [--out csv]`：S6.1 的六维风格因子测量。
/// 因子 = 笔触尺度 / 色阶数 / 边缘强度 / 局部对比 / 色温 / 饱和度；
/// 给了 `--vs` 就按两组算 Cohen's d（|d|≥1 算能分开）。
fn style_cmd(argv: &[String]) {
    use std::io::Write as _;
    use trit_parts::look;

    let mut group_a: Vec<PathBuf> = Vec::new();
    let mut group_b: Vec<PathBuf> = Vec::new();
    let mut cur = &mut group_a;
    let mut skip = false;
    for a in argv.iter().skip(1) {
        if skip {
            skip = false;
            continue;
        }
        match a.as_str() {
            "--vs" => {
                cur = &mut group_b;
                continue;
            }
            "--out" => {
                skip = true;
                continue;
            }
            _ if a.starts_with("--") => continue,
            _ => {}
        }
        let p = PathBuf::from(a);
        if p.is_dir() {
            let mut v: Vec<PathBuf> = std::fs::read_dir(&p)
                .unwrap_or_else(|e| fail(&format!("读不了 {}: {e}", p.display())))
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|q| q.extension().is_some_and(|x| x == "png"))
                .collect();
            v.sort();
            cur.extend(v);
        } else {
            cur.push(p);
        }
    }
    if group_a.is_empty() {
        fail("用法: parts-3d style <A 图/目录…> [--vs <B 图/目录…>] [--out csv]");
    }

    let measure = |files: &[PathBuf]| -> Vec<(String, [f32; 6])> {
        files
            .iter()
            .map(|f| {
                let (w, h, rgb) = trit_parts::data::read_rgb(f).unwrap_or_else(|e| fail(&e));
                let l = look::analyze(&rgb, w, h);
                let name = f
                    .file_stem()
                    .map(|x| x.to_string_lossy().into_owned())
                    .unwrap_or_default();
                // 六个因子：笔触、级数、硬边、对比、色温、饱和
                (
                    name,
                    [l.stroke, l.levels, l.strong, l.contrast, l.warmth, l.sat],
                )
            })
            .collect()
    };
    let a = measure(&group_a);
    let b = measure(&group_b);

    let mut out = std::io::stdout().lock();
    let names = ["笔触", "级数", "硬边", "对比", "色温", "饱和"];
    let _ = writeln!(
        out,
        "{:<22} {:>6} {:>6} {:>6} {:>6} {:>6} {:>6}",
        "样本", "笔触", "级数", "硬边", "对比", "色温", "饱和"
    );
    for (n, v) in a.iter().take(50) {
        let _ = writeln!(
            out,
            "{n:<22} {:>6.2} {:>6.1} {:>6.3} {:>6.2} {:>6.2} {:>6.2}",
            v[0], v[1], v[2], v[3], v[4], v[5]
        );
    }

    let mean_std = |rows: &[(String, [f32; 6])], k: usize| -> (f32, f32) {
        let n = rows.len() as f32;
        let m = rows.iter().map(|(_, v)| v[k]).sum::<f32>() / n;
        let var = rows
            .iter()
            .map(|(_, v)| (v[k] - m) * (v[k] - m))
            .sum::<f32>()
            / n.max(1.0);
        (m, var.sqrt())
    };
    let _ = writeln!(
        out,
        "
A 组 {} 张{}",
        a.len(),
        if b.is_empty() {
            String::new()
        } else {
            format!("；B 组 {} 张", b.len())
        }
    );
    let _ = writeln!(
        out,
        "{:<6} {:>16} {:>16} {:>7}",
        "因子", "A 均值±std", "B 均值±std", "d"
    );
    let mut separable = 0;
    for (k, name) in names.iter().enumerate() {
        let (ma, sa) = mean_std(&a, k);
        if b.is_empty() {
            let _ = writeln!(out, "{name:<6} {ma:>10.3}±{sa:<5.3}");
            continue;
        }
        let (mb, sb) = mean_std(&b, k);
        let pooled = (((sa * sa * a.len() as f32 + sb * sb * b.len() as f32)
            / (a.len() + b.len()).max(1) as f32)
            .sqrt())
        .max(1e-6);
        let d = (ma - mb) / pooled;
        if d.abs() >= 1.0 {
            separable += 1;
        }
        let _ = writeln!(
            out,
            "{name:<6} {ma:>10.3}±{sa:<5.3} {mb:>10.3}±{sb:<5.3} {d:>7.2}"
        );
    }
    if !b.is_empty() {
        let _ = writeln!(out, "可分开的因子：{separable}/6（|d|≥1）");
    }

    if let Some(p) = arg(argv, "--out").map(PathBuf::from) {
        let mut csv = String::from(
            "group,name,stroke,levels,strong,contrast,warmth,sat
",
        );
        for (g, rows) in [("A", &a), ("B", &b)] {
            for (n, v) in rows.iter() {
                csv.push_str(&format!(
                    "{g},{n},{:.3},{:.1},{:.4},{:.4},{:.4},{:.4}
",
                    v[0], v[1], v[2], v[3], v[4], v[5]
                ));
            }
        }
        std::fs::write(&p, csv).unwrap_or_else(|e| fail(&format!("{}: {e}", p.display())));
        let _ = writeln!(out, "因子表已写 {}", p.display());
    }
}

fn masked_psnr(img: &[f32], gt: &[f32], ids: &[u32]) -> f64 {
    let mut a = Vec::new();
    let mut b = Vec::new();
    for (p, &id) in ids.iter().enumerate() {
        if id == 0 {
            continue;
        }
        a.extend_from_slice(&img[p * 3..p * 3 + 3]);
        b.extend_from_slice(&gt[p * 3..p * 3 + 3]);
    }
    trit_parts::splat::psnr(&a, &b)
}

fn avg(v: &[f64]) -> f64 {
    let f: Vec<f64> = v.iter().copied().filter(|x| x.is_finite()).collect();
    f.iter().sum::<f64>() / f.len().max(1) as f64
}

fn usage() -> ! {
    eprintln!(
        "用法:
  parts-3d cloud --seg <npy> --depth <npy> --meta <json> --out <ply>
  parts-3d infer --model <tpm> --scene <dir> --frame N --manifest <json> --out <ply> [--depth <npy>]
  parts-3d eval --model <tpm> --scene <dir> --manifest <json> [--limit N] [--voxel 0.01] [--min-vox 30]
  parts-3d mesh --seg <npy> --depth <npy> --meta <json> --out <ply> [--cell 0.01] [--levels 0,2,4]
  parts-3d fuse --scene <dir> --frames 0,1,2,3 [--eval 4,5,6,7] --out <ply> [--cell 0.01] [--min-votes 1] [--voxel 0.01] [--levels 0,2]
  parts-3d splat --scene <dir> --frames 0,2,4,6 --eval 1,3,5,7 [--cells 0.04,0.02,0.01] [--scale-k 2] [--alpha 0.6]
  parts-3d occ --root <renders 根> --scenes <scenes 根> --train 80,81 --eval 84,85 [--cell 0.01] [--steps 20000] [--selfsup] [--views N] [--tau 0.01]
  parts-3d align --scene <dir> --frames 0,1,2,3,4,5,6,7 [--cell 0.01] [--tau 0.01] [--fill]
  parts-3d obs-split --scene <dir> --frames 0,1,2,3,4,5,6,7 --out <ply> [--cell 0.01] [--tau 0.01]
  parts-3d mesh-tex --scene <dir> --frames 0,1,2,3,4,5,6,7 --out <ply> [--cell 0.01] [--tau 0.01]
  parts-3d tex --kind wood|marble|rust|grid --size 256 --out <ppm>
  parts-3d tex-prior [--iters 30000] [--patch 3|7] [--cond] [--hid 16]
  parts-3d tex-conv  [--iters 20000] [--hidden 8,8] [--cond] [--size 64] [--adam] [--batch K] [--bn]
  parts-3d tex-quilt <源.png> [--out out.ppm] [--size 128] [--patch 12] [--overlap 3] [--seed 1]
  parts-3d tex-stylize <in.png> [--preset cel|oil|epic --strength 0..1] [--contrast K] [--levels N] [--warmth T] [--sat S] [--stroke R] --out o.png
  parts-3d tex-gen   [--iters 200000] [--steps-t 50] [--size 64] [--hid 32] [--out <dir>]
                     [--conv 8,8] [--batch 4] [--adam] [--eps|--v] [--eps-space] [--diag]
                     [--dil N] [--save w.tpc] [--load w.tpc] [--holdout-names A,B]
  parts-3d look <图或目录>… [--out features.csv]"
    );
    std::process::exit(2);
}

fn fail(msg: &str) -> ! {
    eprintln!("{msg}");
    std::process::exit(1);
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    match argv.first().map(String::as_str) {
        Some("cloud") => cloud(&argv),
        Some("mesh") => mesh_cmd(&argv),
        Some("fuse") => fuse_cmd(&argv),
        Some("splat") => splat_cmd(&argv),
        Some("occ") => occ_cmd(&argv),
        Some("align") => align_cmd(&argv),
        Some("obs-split") => obs_split_cmd(&argv),
        Some("mesh-tex") => mesh_tex_cmd(&argv),
        Some("tex") => tex_cmd(&argv),
        Some("tex-prior") => tex_prior_cmd(&argv),
        Some("tex-conv") => tex_conv_cmd(&argv),
        Some("tex-gen") => tex_gen_cmd(&argv),
        Some("tex-quilt") => tex_quilt_cmd(&argv),
        Some("material-data") => material_data_cmd(&argv),
        Some("material-fit") => material_fit_cmd(&argv),
        Some("material-predict") => material_predict_cmd(&argv),
        Some("material-cnn") => material_cnn_cmd(&argv),
        Some("occl-stats") => occl_stats_cmd(&argv),
        Some("tex-stylize") => tex_stylize_cmd(&argv),
        Some("look") => look_cmd(&argv),
        Some("style") => style_cmd(&argv),
        Some("infer") => {
            #[cfg(feature = "gpu")]
            infer(&argv);
            #[cfg(not(feature = "gpu"))]
            fail("本构建未启用 gpu feature");
        }
        Some("infer-ids") => {
            #[cfg(feature = "gpu")]
            infer_ids_cmd(&argv);
            #[cfg(not(feature = "gpu"))]
            fail("本构建未启用 gpu feature");
        }
        Some("eval") => {
            #[cfg(feature = "gpu")]
            eval(&argv);
            #[cfg(not(feature = "gpu"))]
            fail("本构建未启用 gpu feature");
        }
        _ => usage(),
    }
}

//! `parts-3d`：部件 id 图 + 深度 → 3D 点云（PLY），以及逐部件 3D IoU 评估。
//!
//! 四个子命令：
//! - `cloud`：GT seg + depth → 逐部件着色的 `.ply`；
//! - `infer`：模型预测 id + **外部深度**（任意来源，默认取 scene/depth）→ `.ply`（生产路径）；
//! - `eval`：模型预测 id + GT depth → 与 GT 反投影逐部件比对（3D IoU / 质心误差）；
//! - `mesh`：点云 → 实心占用 → Surface Nets 网格，并按 `--levels` 出 LOD 金字塔。

use std::path::{Path, PathBuf};

use trit_parts::data::read_npy_i32;
use trit_parts::geo::{Camera, centroid, clouds, iou3d, read_npy_f32, voxelize, write_ply};

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
    let sg = trit_parts::mesh::Grid {
        occ: solid,
        nx: g.nx,
        ny: g.ny,
        nz: g.nz,
        origin: g.origin,
        cell: g.cell,
    };
    let m = surface_nets(&sg);
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
    let lr: f32 = arg(argv, "--lr")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.03);
    let patch: usize = arg(argv, "--patch")
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);
    let cond = argv.iter().any(|a| a == "--cond");
    let r = (patch / 2) as i32;
    let kinds = ["wood", "marble", "rust", "grid"];
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

    let mut train: Vec<(Vec<f32>, f32)> = Vec::new();
    for (ki, k) in kinds.iter().enumerate() {
        let g = gray(k, seed + ki as u64);
        for y in 0..n {
            for x in 0..n {
                let (f, t) = feats(&g, x, y, if cond { Some(ki) } else { None }, &mut nxt);
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
        let (mut se_in, mut se_out, mut cnt) = (0.0f64, 0.0f64, 0usize);
        for y in 0..n {
            for x in 0..n {
                let (f, clean) = feats(g, x, y, ci, nxt);
                let (logit, _) = mlp.forward(&f);
                let pred = 1.0 / (1.0 + (-logit).exp());
                se_in += f64::from((f[(patch * patch) / 2] - clean).powi(2));
                se_out += f64::from((pred - clean).powi(2));
                cnt += 1;
            }
        }
        10.0 * (1.0 / (se_in / cnt as f64)).log10() * 0.0
            + 10.0 * (1.0 / (se_out / cnt as f64)).log10()
    };
    let mut good = Vec::new();
    let mut wrong = Vec::new();
    for (ki, k) in kinds.iter().enumerate() {
        let g = gray(k, eval_seed + ki as u64);
        let p_ok = psnr_of(&mlp, &g, if cond { Some(ki) } else { None }, &mut nxt);
        let p_bad = if cond {
            psnr_of(&mlp, &g, Some((ki + 1) % kinds.len()), &mut nxt)
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
        let ki = (step * 7919) % kinds.len();
        let sd = seed + ki as u64;
        let clean = gray(kinds[ki], sd);
        let inp = noisy(&clean, &mut nxt);
        let c = if cond { Some(ki) } else { None };
        loss += net.train_step(&inp, &clean, c, n, n, lr) as f64;
        if (step + 1) % 5000 == 0 {
            println!("  step {} loss {:.4}", step + 1, loss / 5000.0);
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
        "三值卷积栈 hidden {:?}，BN {}，条件 {}，{} 步；留出 PSNR 均值 {:.2} dB{}",
        hidden,
        if use_bn { "开" } else { "关" },
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
  parts-3d tex-conv  [--iters 20000] [--hidden 8,8] [--cond] [--size 64]"
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
        Some("infer") => {
            #[cfg(feature = "gpu")]
            infer(&argv);
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

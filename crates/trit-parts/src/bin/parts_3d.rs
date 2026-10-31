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
    use trit_parts::splat::{build, psnr, render};

    let scene = PathBuf::from(arg(argv, "--scene").unwrap_or_else(|| usage()));
    let parse_list = |k: &str, d: Vec<usize>| -> Vec<usize> {
        arg(argv, k)
            .map(|v| v.split(',').filter_map(|s| s.trim().parse().ok()).collect())
            .unwrap_or(d)
    };
    let frames = parse_list("--frames", vec![0, 2, 4, 6]);
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

    println!("cell(m)  体素      网格顶点/面        外壳非流形边   高斯数   PSNR(fg,dB)");
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
        let mut ps = Vec::new();
        for (v, gt) in eval_views.iter().zip(&eval_rgbs) {
            let img = render(&splats, &v.cam, v.w, v.h, [0.0, 0.0, 0.0]);
            // 只在 GT 前景像素上算（地板/背景不在喷溅表示里）
            let mut a = Vec::new();
            let mut b = Vec::new();
            for p in 0..v.w * v.h {
                if v.ids[p] == 0 {
                    continue;
                }
                a.extend_from_slice(&img[p * 3..p * 3 + 3]);
                b.extend_from_slice(&gt[p * 3..p * 3 + 3]);
            }
            ps.push(psnr(&a, &b));
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
        let mean = ps.iter().filter(|v| v.is_finite()).sum::<f64>()
            / ps.iter().filter(|v| v.is_finite()).count().max(1) as f64;
        println!(
            "{cell:.3}     {vox:<9} {}/{}            {}            {n:<6}  {mean:.2}",
            m.verts.len(),
            m.tris.len(),
            non_manifold_edges(&m)
        );
    }
}

fn usage() -> ! {
    eprintln!(
        "用法:
  parts-3d cloud --seg <npy> --depth <npy> --meta <json> --out <ply>
  parts-3d infer --model <tpm> --scene <dir> --frame N --manifest <json> --out <ply> [--depth <npy>]
  parts-3d eval --model <tpm> --scene <dir> --manifest <json> [--limit N] [--voxel 0.01] [--min-vox 30]
  parts-3d mesh --seg <npy> --depth <npy> --meta <json> --out <ply> [--cell 0.01] [--levels 0,2,4]
  parts-3d fuse --scene <dir> --frames 0,1,2,3 [--eval 4,5,6,7] --out <ply> [--cell 0.01] [--min-votes 1] [--voxel 0.01] [--levels 0,2]
  parts-3d splat --scene <dir> --frames 0,2,4,6 --eval 1,3,5,7 [--cells 0.04,0.02,0.01] [--scale-k 2] [--alpha 0.6]"
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

use std::path::{Path, PathBuf};

use trit_parts::data::read_npy_i32;
use trit_parts::geo::{Camera, read_npy_f32};

use super::cli::{arg, fail, frame_path, usage};
use super::look::{avg, masked_psnr};
use super::occ::{close_solid, model_solid};

/// `splat --scene <dir> --frames 0,2,4,6 --eval 1,3,5,7 --cells 0.04,0.02,0.01`：
/// 精度阶梯：每个采样格同时给出「体素/网格/高斯」三档的数量与留出视角 PSNR。
pub(crate) fn splat_cmd(argv: &[String]) {
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

pub(crate) fn view_points(views: &[trit_parts::fuse::View]) -> Vec<[f32; 3]> {
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
pub(crate) fn align_cmd(argv: &[String]) {
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
pub(crate) fn obs_split_cmd(argv: &[String]) {
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

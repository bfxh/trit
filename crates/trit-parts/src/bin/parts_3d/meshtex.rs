use std::path::PathBuf;

use trit_parts::data::read_npy_i32;
use trit_parts::geo::{Camera, read_npy_f32};

use super::cli::{arg, fail, frame_path, usage};
use super::features::part_features_in;
use super::material::load_material_model;
use super::occ::close_solid;
use super::splat::view_points;
use super::texstyle::{materials_by_pass, style_from_argv};

/// `mesh-tex --scene <dir> --frames ... --out <ply>`：网格 + 顶点颜色烘焙。
/// 颜色从最近的高斯取（观测面），没有近邻的顶点标灰（不可见/推断面）。
/// `mesh-tex` 作业文件允许的键（拼错一个就失败，不静默忽略）。
pub(crate) const MESH_TEX_JOB_KEYS: &[&str] = &[
    "scene",
    "out",
    "frames",
    "cell",
    "tau",
    "clean",
    "open",
    "tex",
    "quilt",
    "quilt-size",
    "patch",
    "overlap",
    "quilt-seed",
    "tex-world",
    "atlas",
    "texels",
    "atlas-deg",
    "lscm",
    "style-by-axis",
    "style-by-material",
    "material-model",
    "min-px",
    "preset",
    "strength",
    "contrast",
    "levels",
    "warmth",
    "sat",
    "stroke",
    "obj",
];

pub(crate) fn mesh_tex_cmd(argv: &[String]) {
    use trit_parts::mesh::{non_manifold_edges, surface_nets, write_ply_rgb};
    // `--job file.json`：把作业参数接在命令行**之后**（`arg()` 取第一个匹配 ⇒ 命令行优先）
    let merged: Vec<String> = match arg(argv, "--job") {
        Some(p) => {
            let txt = std::fs::read_to_string(&p).unwrap_or_else(|e| fail(&format!("{p}: {e}")));
            let job = trit_parts::job::parse_job(&txt, MESH_TEX_JOB_KEYS)
                .unwrap_or_else(|e| fail(&format!("作业 {}：{e}", p)));
            println!("作业 {}：{} 个参数（命令行参数优先）", p, job.len());
            argv.iter().cloned().chain(job).collect()
        }
        None => argv.to_vec(),
    };
    let argv: &[String] = &merged;
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
            let tone = argv.iter().any(|a| a == "--tone-match");
            let detrend: usize = arg(argv, "--detrend")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            let relight = argv.iter().any(|a| a == "--relight");
            let relight_plane = argv.iter().any(|a| a == "--relight-plane");
            let out = trit_parts::tex::quilt(
                &src,
                sw,
                sh,
                n,
                n,
                patch,
                ov,
                seed,
                tone,
                detrend,
                relight,
                relight_plane,
            );
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
        // 目标目录不存在时自动建；否则 write_rgb 会以「找不到路径」失败，报错还指不到真正原因
        if let Some(parent) = obj_path.parent() {
            std::fs::create_dir_all(parent)
                .unwrap_or_else(|e| fail(&format!("{}: {e}", parent.display())));
        }
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
        if let Some(lp) = arg(argv, "--light-out") {
            let fr: usize = arg(argv, "--light-frame")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            let (rw, rh, rpx) = trit_parts::data::read_rgb(&frame_path(&scene, "rgb", fr, "png"))
                .unwrap_or_else(|e| fail(&e));
            let (nrm, nshape) =
                trit_parts::geo::read_npy_f32(&frame_path(&scene, "normal", fr, "npy"))
                    .unwrap_or_else(|e| fail(&e));
            let (seg, _) = trit_parts::data::read_npy_i32(&frame_path(&scene, "seg", fr, "npy"))
                .unwrap_or_else(|e| fail(&e));
            if nshape.len() == 3 {
                let mut lum = vec![0.0f32; rw * rh];
                for i in 0..rw * rh {
                    lum[i] = 0.299 * rpx[i * 3] + 0.587 * rpx[i * 3 + 1] + 0.114 * rpx[i * 3 + 2];
                }
                let mask: Vec<bool> = (0..rw * rh).map(|i| seg[i] > 0).collect();
                if let Some((l, k, a)) =
                    trit_parts::light::fit_from_normals(&lum, &nrm, &mask, rw, rh)
                {
                    let json = format!(
                        "{{\n  \"method\": \"lambert-normal-fit\",\n  \"frame\": {fr},\n  \"dir_world\": [{:.6}, {:.6}, {:.6}],\n  \"intensity\": {:.6},\n  \"ambient\": {:.6}\n}}\n",
                        l[0], l[1], l[2], k, a
                    );
                    std::fs::write(&lp, json).unwrap_or_else(|e| fail(&format!("{lp}: {e}")));
                    println!(
                        "光照侧车 → {lp}：方向 ({:.2},{:.2},{:.2}) 强度 {k:.3} 环境 {a:.3}",
                        l[0], l[1], l[2]
                    );
                }
            }
        }
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

use std::path::{Path, PathBuf};

use trit_parts::data::read_npy_i32;
use trit_parts::geo::{Camera, read_npy_f32};

use super::cli::{arg, fail, frame_path, usage};
use super::splat::view_points;

/// 解析 `objects.json`（数组，元素含 kind/center/scale/rot）→ 原语列表。
pub(crate) fn parse_objects(path: &Path) -> Vec<trit_parts::occ::Primitive> {
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
pub(crate) fn occ_cmd(argv: &[String]) {
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

/// 闭合后处理：清边界 → 填实 → 修对角（自举标签与模型输出共用）。
pub(crate) fn close_solid(g: &trit_parts::mesh::Grid, occ: Vec<u8>) -> Vec<u8> {
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
pub(crate) fn model_solid(
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

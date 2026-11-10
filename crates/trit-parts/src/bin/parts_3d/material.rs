use std::path::{Path, PathBuf};

use trit_parts::geo::Camera;

use super::cli::{arg, fail, frame_path, usage};
use super::features::{LOOK_FEATURES, part_features_in};
use super::texstyle::materials_by_pass;

/// `material-data --scene <dir> --frames 0,1,2 --out csv [--min-px 200]`：
/// 按 seg 取每个部件的**掩码裁剪**，算 look 的 22 个描述量 + manifest 材质标签，写 CSV（训练用）。
pub(crate) fn material_data_cmd(argv: &[String]) {
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
pub(crate) fn occl_stats_cmd(argv: &[String]) {
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
pub(crate) fn part_crops_in(
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
pub(crate) fn material_cnn_cmd(argv: &[String]) {
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
pub(crate) struct MaterialModel {
    mu: Vec<f32>,
    sd: Vec<f32>,
    /// 每类权重：`w[k][0]` 是截距，其后是特征权重。
    w: Vec<Vec<f32>>,
}

pub(crate) fn load_material_model(path: &Path) -> MaterialModel {
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
    pub(crate) fn scores(&self, x: &[f32]) -> Vec<f32> {
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
pub(crate) fn material_predict_cmd(argv: &[String]) {
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
pub(crate) fn material_fit_cmd(argv: &[String]) {
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

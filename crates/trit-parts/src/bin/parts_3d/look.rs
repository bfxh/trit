use std::path::PathBuf;

use super::cli::{arg, fail};

/// `look <图或目录>… [--out csv]`：量「观感」——亮度/对比/饱和/色相熵/细节/平涂/雾感/发光/UI 边条。
/// 启发式描述量，不训练；同一输入两次跑逐位一致（见 `look` 模块单测）。
pub(crate) fn look_cmd(argv: &[String]) {
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
pub(crate) fn fit_rgb(w0: usize, h0: usize, pix: &[f32], n: usize) -> Vec<f32> {
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
pub(crate) fn style_cmd(argv: &[String]) {
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

pub(crate) fn masked_psnr(img: &[f32], gt: &[f32], ids: &[u32]) -> f64 {
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

pub(crate) fn avg(v: &[f64]) -> f64 {
    let f: Vec<f64> = v.iter().copied().filter(|x| x.is_finite()).collect();
    f.iter().sum::<f64>() / f.len().max(1) as f64
}

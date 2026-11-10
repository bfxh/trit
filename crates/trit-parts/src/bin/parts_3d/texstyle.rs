use super::cli::{arg, fail};

pub(crate) fn tex_quilt_cmd(argv: &[String]) {
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
    let tone = argv.iter().any(|a| a == "--tone-match");
    let detrend: usize = arg(argv, "--detrend")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let relight = argv.iter().any(|a| a == "--relight");
    let relight_plane = argv.iter().any(|a| a == "--relight-plane");
    let out = quilt(
        &rgb,
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
pub(crate) fn materials_by_pass(manifest: &str) -> Vec<Option<String>> {
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
pub(crate) fn style_from_argv(argv: &[String]) -> Option<trit_parts::tex::Style> {
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
pub(crate) fn tex_stylize_cmd(argv: &[String]) {
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

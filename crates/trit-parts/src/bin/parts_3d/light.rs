use std::path::PathBuf;

use trit_parts::geo::Camera;

use super::cli::{arg, fail, frame_path, usage};

/// `light-est <图…>`：单图光照估计（方位角/掠射/亮度/色温），并列出 trit-look 的代理量做对照。
pub(crate) fn light_est_cmd(argv: &[String]) {
    use trit_parts::light;
    let mut files: Vec<PathBuf> = Vec::new();
    let mut skip = false;
    for a in argv.iter().skip(1) {
        if skip {
            skip = false;
            continue;
        }
        if a.starts_with("--") {
            skip = matches!(a.as_str(), "--out");
            continue;
        }
        let p = PathBuf::from(a);
        if p.is_dir() {
            let mut v: Vec<PathBuf> = std::fs::read_dir(&p)
                .map(|it| it.filter_map(|e| e.ok()).map(|e| e.path()))
                .into_iter()
                .flatten()
                .filter(|p| p.extension().is_some_and(|x| x == "png" || x == "jpg"))
                .collect();
            v.sort();
            files.extend(v);
        } else {
            files.push(p);
        }
    }
    println!(
        "{:<34} {:>9} {:>8} {:>8} {:>8} {:>9}",
        "图", "方位角°", "掠射", "平均亮度", "暖差", "look.key_dir"
    );
    for f in &files {
        let (w, h, px) = trit_parts::data::read_rgb(f).unwrap_or_else(|e| fail(&e));
        let e = light::estimate(&px, w, h);
        let l = trit_parts::look::analyze(&px, w, h);
        println!(
            "{:<34} {:>9.1} {:>8.3} {:>8.3} {:>8.3} {:>9.1}",
            f.file_name().and_then(|x| x.to_str()).unwrap_or("?"),
            e.azimuth_deg,
            e.grazing,
            e.mean_luma,
            e.warmth,
            l.key_dir
        );
    }
}

/// `light-relight-normals --scene <dir> --frame N --az-deg <Δ> --out <png>`：
/// 用法线重打光（光方向绕 z 旋转 Δ 度），输出后可用 `light-fit` 复核。
pub(crate) fn light_relight_normals_cmd(argv: &[String]) {
    use trit_parts::light;
    let scene = PathBuf::from(arg(argv, "--scene").unwrap_or_else(|| usage()));
    let frame: usize = arg(argv, "--frame")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let deg: f32 = arg(argv, "--az-deg")
        .and_then(|v| v.parse().ok())
        .unwrap_or(30.0);
    let out_p = arg(argv, "--out").unwrap_or_else(|| usage());
    let (w, h, rgb) = trit_parts::data::read_rgb(&frame_path(&scene, "rgb", frame, "png"))
        .unwrap_or_else(|e| fail(&e));
    let (nrm, nshape) = trit_parts::geo::read_npy_f32(&frame_path(&scene, "normal", frame, "npy"))
        .unwrap_or_else(|e| fail(&e));
    let (seg, _) = trit_parts::data::read_npy_i32(&frame_path(&scene, "seg", frame, "npy"))
        .unwrap_or_else(|e| fail(&e));
    if nshape.len() != 3 {
        fail("法线图应为 HxWx3");
    }
    let mut lum = vec![0.0f32; w * h];
    for i in 0..w * h {
        lum[i] = 0.299 * rgb[i * 3] + 0.587 * rgb[i * 3 + 1] + 0.114 * rgb[i * 3 + 2];
    }
    let mask: Vec<bool> = (0..w * h).map(|i| seg[i] > 0).collect();
    let Some((l, k, a)) = light::fit_from_normals(&lum, &nrm, &mask, w, h) else {
        fail("拟合失败");
    };
    let ln = light::rotate_light(l, deg);
    let out = light::relight_normals(&rgb, &nrm, w, h, ln, k, a);
    trit_parts::data::write_rgb(std::path::Path::new(&out_p), w, h, &out)
        .unwrap_or_else(|e| fail(&e));
    let after = light::estimate(&out, w, h);
    println!(
        "光方向旋转 {:+.1}°：l=({:.2},{:.2},{:.2}) → ({:.2},{:.2},{:.2})；新图平面估计方位 {:.1}° ⇒ {}",
        deg, l[0], l[1], l[2], ln[0], ln[1], ln[2], after.azimuth_deg, out_p
    );
}

/// `light-fit --scene <dir> --frame N [--az-deg A --el-deg E]`：
/// 用法线 + 亮度线性朗伯拟合估计光方向（相机坐标系），给了真值就报角度误差（相机/世界两种约定都算）。
pub(crate) fn light_fit_cmd(argv: &[String]) {
    use trit_parts::light;
    let scene = PathBuf::from(arg(argv, "--scene").unwrap_or_else(|| usage()));
    let frame: usize = arg(argv, "--frame")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let (w, h, rgb) = trit_parts::data::read_rgb(&frame_path(&scene, "rgb", frame, "png"))
        .unwrap_or_else(|e| fail(&e));
    let (nrm, nshape) = trit_parts::geo::read_npy_f32(&frame_path(&scene, "normal", frame, "npy"))
        .unwrap_or_else(|e| fail(&e));
    let (seg, sshape) = trit_parts::data::read_npy_i32(&frame_path(&scene, "seg", frame, "npy"))
        .unwrap_or_else(|e| fail(&e));
    if nshape.len() != 3 || nshape[0] != h || nshape[1] != w || sshape[0] != h || sshape[1] != w {
        fail("rgb/normal/seg 尺寸不一致");
    }
    let mut lum = vec![0.0f32; w * h];
    for i in 0..w * h {
        lum[i] = 0.299 * rgb[i * 3] + 0.587 * rgb[i * 3 + 1] + 0.114 * rgb[i * 3 + 2];
    }
    let mask: Vec<bool> = (0..w * h).map(|i| seg[i] > 0).collect();
    let Some((l, k, a)) = light::fit_from_normals(&lum, &nrm, &mask, w, h) else {
        fail("拟合失败（前景像素不足或法线退化）");
    };
    println!(
        "估计光方向（相机系）l=({:.3}, {:.3}, {:.3})，强度 k={k:.3}，环境 a={a:.3}",
        l[0], l[1], l[2]
    );
    if let (Some(az), Some(el)) = (
        arg(argv, "--az-deg").and_then(|v| v.parse::<f32>().ok()),
        arg(argv, "--el-deg").and_then(|v| v.parse::<f32>().ok()),
    ) {
        let (ar, er) = (az.to_radians(), el.to_radians());
        let lw = [er.cos() * ar.cos(), er.cos() * ar.sin(), er.sin()];
        // T_wc：相机→世界；R_cw = R_wc^T
        let cam =
            Camera::load(&frame_path(&scene, "meta", frame, "json")).unwrap_or_else(|e| fail(&e));
        let r = [
            [cam.t_cw[0][0], cam.t_cw[1][0], cam.t_cw[2][0]],
            [cam.t_cw[0][1], cam.t_cw[1][1], cam.t_cw[2][1]],
            [cam.t_cw[0][2], cam.t_cw[1][2], cam.t_cw[2][2]],
        ];
        let lc = [
            (r[0][0] * lw[0] as f64 + r[0][1] * lw[1] as f64 + r[0][2] * lw[2] as f64) as f32,
            (r[1][0] * lw[0] as f64 + r[1][1] * lw[1] as f64 + r[1][2] * lw[2] as f64) as f32,
            (r[2][0] * lw[0] as f64 + r[2][1] * lw[1] as f64 + r[2][2] * lw[2] as f64) as f32,
        ];
        println!(
            "真值 az={az}° el={el}°：与相机系误差 {:.1}°，与世界系误差 {:.1}°",
            light::angle_between(l, lc),
            light::angle_between(l, lw)
        );
    }
}

/// `light-relight <in> --az-deg <θ> --out <out.png>`：平面近似重打光（旋转光源方位角）。
pub(crate) fn light_relight_cmd(argv: &[String]) {
    use trit_parts::light;
    let Some(p) = argv.get(1).filter(|a| !a.starts_with("--")) else {
        fail("用法: parts-3d light-relight <in.png> --az-deg 30 --out o.png");
    };
    let deg: f32 = arg(argv, "--az-deg")
        .and_then(|v| v.parse().ok())
        .unwrap_or(30.0);
    let out_p = arg(argv, "--out").unwrap_or_else(|| usage());
    let (w, h, px) =
        trit_parts::data::read_rgb(std::path::Path::new(p)).unwrap_or_else(|e| fail(&e));
    let before = light::estimate(&px, w, h);
    let out = light::relight(&px, w, h, deg);
    let after = light::estimate(&out, w, h);
    println!(
        "方位角 {:.1}° → {:.1}°（请求 {:+.1}°）",
        before.azimuth_deg, after.azimuth_deg, deg
    );
    trit_parts::data::write_rgb(std::path::Path::new(&out_p), w, h, &out)
        .unwrap_or_else(|e| fail(&e));
    println!("写出 {out_p}");
}

//! 训练数据读取：PNG（RGB）→ f32 NHWC，`.npy`（int32 部件 id）→ 三进制平面 + valid。
//!
//! 数据来自 `big-r*/{rgb,seg}`，分辨率已是 384²，不做缩放（与 Python 训练同一份帧）。
//! 编码口径与 `part_index.encode` 一致：`valid = id > 0`（背景与未知都不可信），
//! `plane[i] = ((id−1) / 3^(k−1−i)) % 3`，valid=0 处平面置 0。

use std::fs::File;
use std::io::BufWriter;
mod npy;
mod png_read;

use png_read::read_png_rgb;

pub use npy::{read_npy_i32, write_npy_i32};

use std::path::{Path, PathBuf};

/// 一帧的输入与标签。
pub struct Sample {
    /// RGB，NHWC，`h*w*3`，值域 `[0,1]`。
    pub rgb: Vec<f32>,
    /// `k` 条平面，每条 `h*w`，取值 0/1/2。
    pub planes: Vec<Vec<u8>>,
    /// 部件掩码，`h*w`，0/1。
    pub valid: Vec<u8>,
}

/// 一个场景目录（含 `rgb/` 与 `seg/`）。
pub fn frames(scene: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(scene.join("rgb"))
        .map(|it| {
            it.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "png"))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// 读一帧：RGB + 同名的 `seg/*.npy`。
pub fn load_frame(scene: &Path, rgb: &Path, k: usize) -> Result<Sample, String> {
    load_frame_sub(scene, rgb, k, "seg")
}

/// 同 `load_frame`，但标签取自 `<scene>/<seg_sub>/*.npy`（如实例标签 `inst`）。
pub fn load_frame_sub(scene: &Path, rgb: &Path, k: usize, seg_sub: &str) -> Result<Sample, String> {
    let (w, h, pix) = read_png_rgb(rgb)?;
    let stem = rgb
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or("文件名非法")?;
    let (labels, shape) = read_npy_i32(&scene.join(seg_sub).join(format!("{stem}.npy")))?;
    if shape.len() != 2 || shape[1] != w || shape[0] != h {
        return Err(format!("{stem}: PNG {w}x{h} 与 seg {shape:?} 不一致"));
    }
    let (planes, valid) = encode(&labels, w, h, k);
    Ok(Sample {
        rgb: pix,
        planes,
        valid,
    })
}

/// id 图 → (k 条平面, valid)。
/// GT 深度 → 视差三值平面（`dplanes` 条，`3^dplanes` 级，区间 `[lo, hi]`）。
/// 只在 `seg > 0` 且深度合法的像素上有标签。
pub fn depth_planes(
    scene: &Path,
    file: &Path,
    npix: usize,
    dplanes: usize,
    lo: f32,
    hi: f32,
) -> Result<(Vec<Vec<u8>>, Vec<u8>), String> {
    let levels = crate::planes::levels_checked(dplanes)?;
    let stem = file
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .ok_or("文件名缺 stem")?;
    let (dep, _) = crate::geo::read_npy_f32(&scene.join("depth").join(format!("{stem}.npy")))?;
    let (seg, _) = read_npy_i32(&scene.join("seg").join(format!("{stem}.npy")))?;
    if dep.len() < npix || seg.len() < npix {
        return Err("深度/分割比图像小".into());
    }
    let span = (hi - lo).max(1e-6);
    let mut planes = vec![vec![0u8; npix]; dplanes];
    let mut valid = vec![0u8; npix];
    for p in 0..npix {
        let d = dep[p];
        if seg[p] <= 0 || !(d.is_finite() && d > 0.05 && d < 50.0) {
            continue;
        }
        let t = ((1.0 / d - lo) / span).clamp(0.0, 1.0);
        let lvl = ((t * levels as f32).round() as usize).min(levels);
        valid[p] = 1;
        for (i, plane) in planes.iter_mut().enumerate() {
            plane[p] = ((lvl / crate::planes::pow3(dplanes - 1 - i)) % 3) as u8;
        }
    }
    Ok((planes, valid))
}

pub fn encode(labels: &[i32], w: usize, h: usize, k: usize) -> (Vec<Vec<u8>>, Vec<u8>) {
    let n = w * h;
    let mut planes = vec![vec![0u8; n]; k];
    let mut valid = vec![0u8; n];
    for p in 0..n {
        let id = labels[p];
        if id <= 0 {
            continue;
        }
        valid[p] = 1;
        let mut t = (id - 1) as u32;
        for i in (0..k).rev() {
            planes[i][p] = (t % 3) as u8;
            t /= 3;
        }
    }
    (planes, valid)
}

/// 双线性把 RGB 缩放到 `res²`（外部图片尺寸任意；已是目标尺寸时原样返回）。
pub fn resize_rgb(w: usize, h: usize, pix: &[f32], res: usize) -> Vec<f32> {
    if w == res && h == res {
        return pix.to_vec();
    }
    let mut out = vec![0f32; crate::dims::pixels(res, res) * 3];
    for y in 0..res {
        let fy = (y as f32 + 0.5) * h as f32 / res as f32 - 0.5;
        let y0 = fy.floor().max(0.0) as usize;
        let y1 = (y0 + 1).min(h.saturating_sub(1));
        let ty = (fy - y0 as f32).clamp(0.0, 1.0);
        for x in 0..res {
            let fx = (x as f32 + 0.5) * w as f32 / res as f32 - 0.5;
            let x0 = fx.floor().max(0.0) as usize;
            let x1 = (x0 + 1).min(w.saturating_sub(1));
            let tx = (fx - x0 as f32).clamp(0.0, 1.0);
            for c in 0..3 {
                let a = pix[(y0 * w + x0) * 3 + c] * (1.0 - tx) + pix[(y0 * w + x1) * 3 + c] * tx;
                let b = pix[(y1 * w + x0) * 3 + c] * (1.0 - tx) + pix[(y1 * w + x1) * 3 + c] * tx;
                out[(y * res + x) * 3 + c] = a * (1.0 - ty) + b * ty;
            }
        }
    }
    out
}

/// 只读 RGB（推理用，不需要 seg 标签）。
pub fn read_rgb(path: &Path) -> Result<(usize, usize, Vec<f32>), String> {
    // PNG 走本仓解码器（与训练一致）；其它格式（JPEG）交给 image crate
    let magic = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if magic.starts_with(&[0x89, b'P', b'N', b'G']) {
        return read_png_rgb(path);
    }
    let img = image::load_from_memory(&magic)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .to_rgb8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    let mut out = Vec::with_capacity(w * h * 3);
    for p in img.pixels() {
        out.push(f32::from(p[0]) / 255.0);
        out.push(f32::from(p[1]) / 255.0);
        out.push(f32::from(p[2]) / 255.0);
    }
    Ok((w, h, out))
}

/// 写 PNG（RGB，`pix` 为交错 f32 0..1）。与 `read_rgb` 互逆（量化误差 ≤1/255）。
pub fn write_rgb(path: &Path, w: usize, h: usize, pix: &[f32]) -> Result<(), String> {
    if pix.len() != w * h * 3 {
        return Err(format!(
            "{}: 像素数 {} != {}×{}×3",
            path.display(),
            pix.len(),
            w,
            h
        ));
    }
    let file = File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut enc = png::Encoder::new(BufWriter::new(file), w as u32, h as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let mut wr = enc.write_header().map_err(|e| e.to_string())?;
    let bytes: Vec<u8> = pix
        .iter()
        .map(|v| (v.clamp(0.0, 1.0) * 255.0) as u8)
        .collect();
    wr.write_image_data(&bytes).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_matches_base3_digits() {
        // k=2: id 1→[0,0], 2→[0,1], 3→[0,2], 4→[1,0]；0 与 -1 都无效
        let labels: Vec<i32> = vec![1, 2, 3, 4, 0, -1];
        let (planes, valid) = encode(&labels, 3, 2, 2);
        assert_eq!(valid, vec![1, 1, 1, 1, 0, 0]);
        assert_eq!(planes[0], vec![0, 0, 0, 1, 0, 0]);
        assert_eq!(planes[1], vec![0, 1, 2, 0, 0, 0]);
    }

    #[test]
    fn png_write_read_roundtrip() {
        let (w, h) = (7usize, 5usize);
        let pix: Vec<f32> = (0..w * h * 3)
            .map(|i| ((i * 37) % 256) as f32 / 255.0)
            .collect();
        let p = std::env::temp_dir().join("trit_parts_png_roundtrip.png");
        write_rgb(&p, w, h, &pix).unwrap();
        let (w2, h2, back) = read_rgb(&p).unwrap();
        assert_eq!((w2, h2), (w, h));
        for (a, b) in pix.iter().zip(&back) {
            assert!((a - b).abs() <= 1.0 / 255.0 + 1e-6, "量化误差应 ≤1/255");
        }
        std::fs::remove_file(&p).ok();
    }
}

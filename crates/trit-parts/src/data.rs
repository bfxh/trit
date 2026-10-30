//! 训练数据读取：PNG（RGB）→ f32 NHWC，`.npy`（int32 部件 id）→ 三进制平面 + valid。
//!
//! 数据来自 `big-r*/{rgb,seg}`，分辨率已是 384²，不做缩放（与 Python 训练同一份帧）。
//! 编码口径与 `part_index.encode` 一致：`valid = id > 0`（背景与未知都不可信），
//! `plane[i] = ((id−1) / 3^(k−1−i)) % 3`，valid=0 处平面置 0。

use std::fs::File;
use std::io::BufReader;
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
    let (w, h, pix) = read_png_rgb(rgb)?;
    let stem = rgb
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or("文件名非法")?;
    let (labels, shape) = read_npy_i32(&scene.join("seg").join(format!("{stem}.npy")))?;
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
    let stem = file
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .ok_or("文件名缺 stem")?;
    let (dep, _) = crate::geo::read_npy_f32(&scene.join("depth").join(format!("{stem}.npy")))?;
    let (seg, _) = read_npy_i32(&scene.join("seg").join(format!("{stem}.npy")))?;
    if dep.len() < npix || seg.len() < npix {
        return Err("深度/分割比图像小".into());
    }
    let levels = 3usize.pow(dplanes as u32) - 1;
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
            plane[p] = ((lvl / 3usize.pow((dplanes - 1 - i) as u32)) % 3) as u8;
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

/// 只读 RGB（推理用，不需要 seg 标签）。
pub fn read_rgb(path: &Path) -> Result<(usize, usize, Vec<f32>), String> {
    read_png_rgb(path)
}

/// 读 PNG，返回 `(宽, 高, RGB 的 f32 NHWC)`。
fn read_png_rgb(path: &Path) -> Result<(usize, usize, Vec<f32>), String> {
    let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut decoder = png::Decoder::new(BufReader::new(file));
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    let (w, h) = (info.width as usize, info.height as usize);
    let channels = match info.color_type {
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Indexed => return Err("索引色 PNG 未展开".into()),
    };
    let mut out = Vec::with_capacity(w * h * 3);
    for p in 0..w * h {
        let base = p * channels;
        let (r, g, b) = match channels {
            1 => (buf[base], buf[base], buf[base]),
            2 => (buf[base], buf[base], buf[base]),
            4 => (buf[base], buf[base + 1], buf[base + 2]),
            _ => (buf[base], buf[base + 1], buf[base + 2]),
        };
        out.push(f32::from(r) / 255.0);
        out.push(f32::from(g) / 255.0);
        out.push(f32::from(b) / 255.0);
    }
    Ok((w, h, out))
}

/// 读 `.npy`（仅支持小端整型 2 维数组），返回 `(数据, 形状)`。
pub fn read_npy_i32(path: &Path) -> Result<(Vec<i32>, Vec<usize>), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.len() < 10 || &bytes[0..6] != b"\x93NUMPY" {
        return Err("不是 .npy 文件".into());
    }
    let major = bytes[6];
    let (hlen, start) = if major == 1 {
        (u16::from_le_bytes([bytes[8], bytes[9]]) as usize, 10)
    } else {
        (
            u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize,
            12,
        )
    };
    let header = std::str::from_utf8(&bytes[start..start + hlen]).map_err(|_| "npy 头非 UTF-8")?;
    let descr = field(header, "descr").ok_or("npy 缺 descr")?;
    if descr != "<i4" && descr != "|i4" {
        return Err(format!("只支持 int32，得到 {descr}"));
    }
    let shape: Vec<usize> = field(header, "shape")
        .ok_or("npy 缺 shape")?
        .trim_matches(|c| c == '(' || c == ')' || c == ',' || c == ' ')
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();
    let n: usize = shape.iter().product();
    let body = &bytes[start + hlen..];
    if body.len() < n * 4 {
        return Err("npy 数据长度不足".into());
    }
    let data = body[..n * 4]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| i32::from_le_bytes(*c))
        .collect();
    Ok((data, shape))
}

fn field<'a>(header: &'a str, key: &str) -> Option<&'a str> {
    let i = header.find(&format!("'{key}'"))? + key.len() + 2;
    let rest = header[i..].trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();
    if let Some(r) = rest.strip_prefix('\'') {
        let b = r.find('\'')?;
        Some(&r[..b])
    } else {
        let end = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '(' || c == ')' || c == ',' || c == ' '))
            .unwrap_or(rest.len());
        Some(rest[..end].trim_end())
    }
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
    fn npy_roundtrip_shape() {
        // 手工构造一个 2×3 int32 npy（v1.0 头）
        let header = "{'descr': '<i4', 'fortran_order': False, 'shape': (2, 3), }";
        let mut h = header.as_bytes().to_vec();
        while !(10 + h.len()).is_multiple_of(64) {
            h.push(b' ');
        }
        h.push(b'\n');
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"\x93NUMPY\x01\x00");
        bytes.extend_from_slice(&(h.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&h);
        for v in [1i32, 2, 3, 4, 5, 6] {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        let dir = std::env::temp_dir().join("trit-parts-npy-test");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.npy");
        std::fs::write(&p, &bytes).unwrap();
        let (data, shape) = read_npy_i32(&p).unwrap();
        assert_eq!(shape, vec![2, 3]);
        assert_eq!(data, vec![1, 2, 3, 4, 5, 6]);
        std::fs::remove_file(&p).ok();
    }
}

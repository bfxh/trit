use std::path::Path;

use super::cli::{fail, frame_path};

/// `tex-quilt <源.png> [--out out.ppm] [--size 128] [--patch 12] [--overlap 3] [--seed 1]`：
/// 非参数 patch 合成（搬真实 patch，结构/统计由构造保证；与 `tex-gen` 的学出来的合成对照用）。
/// look 的 22 个描述量（顺序固定；CSV 与训练都用它）。
pub(crate) const LOOK_FEATURES: [&str; 22] = [
    "luma",
    "contrast",
    "sat",
    "hue_entropy",
    "edge",
    "fine",
    "strong",
    "flat",
    "levels",
    "haze",
    "emissive",
    "bars",
    "warmth",
    "key_dir",
    "key_fill_dwarm",
    "highlights",
    "hf1",
    "hf2",
    "hf4",
    "border",
    "stroke",
    "coherence",
];

pub(crate) fn look_vec(l: &trit_parts::look::Look) -> Vec<f32> {
    vec![
        l.luma,
        l.contrast,
        l.sat,
        l.hue_entropy,
        l.edge,
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
        l.stroke,
        l.coherence,
    ]
}

/// 取一帧里所有可见部件（像素数 ≥ min_px）的 22 维特征：`(部件号, 特征)`。
/// 掩码外填该部件均值色，避免背景污染特征。
pub(crate) fn part_features_in(scene: &Path, frame: usize, min_px: usize) -> Vec<(u32, Vec<f32>)> {
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
        let mut crop = vec![0.0f32; cw * ch * 3];
        for y in 0..ch {
            for x in 0..cw {
                let inside = ids[(y0 + y) * w + x0 + x] == id as i32;
                for c in 0..3 {
                    crop[(y * cw + x) * 3 + c] = if inside {
                        rgb[((y0 + y) * w + x0 + x) * 3 + c]
                    } else {
                        mean[c]
                    };
                }
            }
        }
        let l = trit_parts::look::analyze(&crop, cw, ch);
        out.push((id, look_vec(&l)));
    }
    out
}

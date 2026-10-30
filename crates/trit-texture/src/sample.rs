//! 压缩域纹理采样：**不整图解码**，按需从块里取纹素再做过滤。
//!
//! 引擎侧只把块流传进显存，片元着色器采样时现解码；这里是同一套语义的 CPU
//! 参考实现（也是 GPU 侧的对拍基线）。坐标约定与图形 API 一致：
//! `u, v ∈ [0, 1]`，**纹素中心**在 `(i + 0.5) / 尺寸`，越界按 **clamp-to-edge**。
//!
//! 只在 3 级调色板上取值 ⇒ 双线性必须解 4 个纹素（可能跨 2×2 个块），
//! 但不需要把整张图展开成 RGBA：一次采样最多碰 4 个块。块形状**不写死**，
//! 统一取 `BLOCK_WIDTH` / `BLOCK_HEIGHT`（当前 4×10，见 lib.rs）。

use crate::ttx2::TtxDocument;
use crate::{
    BLOCK_HEIGHT, BLOCK_WIDTH, TRIT_PAYLOAD_BYTES, TritTexture, palette_from_endpoints,
    unpack_trits,
};
use trit_core::value::Ternary;

/// 取一个纹素的 RGBA（最近邻，越界 clamp-to-edge）。TT3 不存 alpha，恒 255。
pub fn texel(texture: &TritTexture, x: usize, y: usize) -> [u8; 4] {
    let x = x.min(texture.width.saturating_sub(1));
    let y = y.min(texture.height.saturating_sub(1));
    let block_cols = texture.width / BLOCK_WIDTH;
    let block_index = (y / BLOCK_HEIGHT) * block_cols + (x / BLOCK_WIDTH);
    let block = &texture.blocks[block_index];
    let e0 = u16::from_be_bytes([block[0], block[1]]);
    let e1 = u16::from_be_bytes([block[2], block[3]]);
    let mut payload = [0u8; TRIT_PAYLOAD_BYTES];
    payload.copy_from_slice(&block[4..]);
    let indices = unpack_trits(&payload);
    let palette = palette_from_endpoints(e0, e1);
    let local = (y % BLOCK_HEIGHT) * BLOCK_WIDTH + (x % BLOCK_WIDTH);
    let slot = match indices[local] {
        Ternary::False => 0,
        Ternary::Unknown => 1,
        Ternary::True => 2,
    };
    let (r, g, b) = palette[slot];
    [r, g, b, 255]
}

/// 双线性采样（`u, v ∈ [0,1]`，纹素中心约定，clamp-to-edge）。
///
/// 返回值与 [`crate::decode_rgba`] 同一色彩空间（8 位 RGB，alpha 恒 255）；
/// 权重在 f32 里算完再四舍五入回 8 位。
pub fn sample_bilinear(texture: &TritTexture, u: f32, v: f32) -> [u8; 4] {
    if texture.width == 0 || texture.height == 0 || texture.blocks.is_empty() {
        return [0, 0, 0, 255];
    }
    let fx = u.clamp(0.0, 1.0) * texture.width as f32 - 0.5;
    let fy = v.clamp(0.0, 1.0) * texture.height as f32 - 0.5;
    let x0 = fx.floor();
    let y0 = fy.floor();
    let tx = fx - x0;
    let ty = fy - y0;
    let w = texture.width as i64;
    let h = texture.height as i64;
    let xi = |i: i64| i.clamp(0, w - 1) as usize;
    let yi = |i: i64| i.clamp(0, h - 1) as usize;
    let (ix0, ix1) = (xi(x0 as i64), xi(x0 as i64 + 1));
    let (iy0, iy1) = (yi(y0 as i64), yi(y0 as i64 + 1));
    let p00 = texel(texture, ix0, iy0);
    let p10 = texel(texture, ix1, iy0);
    let p01 = texel(texture, ix0, iy1);
    let p11 = texel(texture, ix1, iy1);
    let mut out = [0u8; 4];
    for c in 0..3 {
        let top = p00[c] as f32 * (1.0 - tx) + p10[c] as f32 * tx;
        let bottom = p01[c] as f32 * (1.0 - tx) + p11[c] as f32 * tx;
        out[c] = (top * (1.0 - ty) + bottom * ty).round().clamp(0.0, 255.0) as u8;
    }
    out[3] = 255;
    out
}

impl TtxDocument {
    /// mip 链（从细到粗）；`kind=MOD` 返回 `None`。
    pub fn levels(&self) -> Option<&[TritTexture]> {
        match self {
            TtxDocument::Texture { levels } => Some(levels),
            TtxDocument::Model(_) => None,
        }
    }

    /// 三线性采样：按 `lod` 在相邻两级各做一次双线性再混合。
    ///
    /// `lod` 会被夹到 `[0, 级数-1]`；只有一级时退化为该级的双线性。
    /// 非纹理文档返回 `None`（不静默给 0）。
    pub fn sample_trilinear(&self, u: f32, v: f32, lod: f32) -> Option<[u8; 4]> {
        let levels = self.levels()?;
        if levels.is_empty() {
            return None;
        }
        let last = (levels.len() - 1) as f32;
        let lod = if lod.is_nan() {
            0.0
        } else {
            lod.clamp(0.0, last)
        };
        let l0 = lod.floor() as usize;
        let l1 = (l0 + 1).min(levels.len() - 1);
        let t = lod - l0 as f32;
        let a = sample_bilinear(&levels[l0], u, v);
        if l1 == l0 || t <= 0.0 {
            return Some(a);
        }
        let b = sample_bilinear(&levels[l1], u, v);
        let mut out = [0u8; 4];
        for c in 0..3 {
            out[c] = (a[c] as f32 * (1.0 - t) + b[c] as f32 * t)
                .round()
                .clamp(0.0, 255.0) as u8;
        }
        out[3] = 255;
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::convert::rgba_to_ttx2_mips;
    use crate::ttx2::from_ttx2;

    /// 造一张 16×20（4×10 块对齐）、每块颜色不同的图（块边界两侧差异大，能抓错块）。
    fn checker(width: usize, height: usize) -> Vec<u8> {
        let mut px = vec![0u8; width * height * 4];
        for y in 0..height {
            for x in 0..width {
                let o = (y * width + x) * 4;
                let bx = (x / BLOCK_WIDTH) as u8;
                let by = (y / BLOCK_HEIGHT) as u8;
                px[o] = bx.wrapping_mul(37);
                px[o + 1] = by.wrapping_mul(53);
                px[o + 2] = ((x + y) % 7) as u8 * 30;
                px[o + 3] = 255;
            }
        }
        px
    }

    fn doc() -> TtxDocument {
        let (w, h) = (16usize, 20usize);
        let px = checker(w, h);
        let bytes = rgba_to_ttx2_mips(&px, w, h).expect("encode");
        from_ttx2(&bytes).expect("parse")
    }

    #[test]
    fn texel_matches_full_decode() {
        let d = doc();
        let levels = d.levels().unwrap();
        let tex = &levels[0];
        let rgba = crate::decode_rgba(tex).expect("decode");
        for y in 0..tex.height {
            for x in 0..tex.width {
                let o = (y * tex.width + x) * 4;
                assert_eq!(
                    texel(tex, x, y),
                    [rgba[o], rgba[o + 1], rgba[o + 2], 255],
                    "纹素 ({x},{y}) 与整图解码不一致"
                );
            }
        }
    }

    #[test]
    fn bilinear_at_texel_center_returns_texel() {
        let d = doc();
        let tex = &d.levels().unwrap()[0];
        for y in 0..tex.height {
            for x in 0..tex.width {
                let u = (x as f32 + 0.5) / tex.width as f32;
                let v = (y as f32 + 0.5) / tex.height as f32;
                assert_eq!(
                    sample_bilinear(tex, u, v),
                    texel(tex, x, y),
                    "纹素中心 ({x},{y}) 双线性应等于该纹素"
                );
            }
        }
    }

    #[test]
    fn bilinear_midpoint_is_rounded_average() {
        let d = doc();
        let tex = &d.levels().unwrap()[0];
        let (x, y) = (5usize, 3usize);
        let u = (x as f32 + 1.0) / tex.width as f32;
        let v = (y as f32 + 0.5) / tex.height as f32;
        let a = texel(tex, x, y);
        let b = texel(tex, x + 1, y);
        let got = sample_bilinear(tex, u, v);
        for c in 0..3 {
            let want = ((a[c] as f32 + b[c] as f32) / 2.0).round() as u8;
            assert_eq!(got[c], want, "通道 {c} 中点应为四舍五入均值");
        }
    }

    #[test]
    fn clamp_to_edge() {
        let d = doc();
        let tex = &d.levels().unwrap()[0];
        assert_eq!(sample_bilinear(tex, 0.0, 0.0), texel(tex, 0, 0));
        assert_eq!(
            sample_bilinear(tex, 1.0, 1.0),
            texel(tex, tex.width - 1, tex.height - 1)
        );
    }

    #[test]
    fn trilinear_at_integer_lod_is_bilinear() {
        let d = doc();
        let levels = d.levels().unwrap();
        for (l, level) in levels.iter().enumerate() {
            let got = d.sample_trilinear(0.31, 0.67, l as f32).unwrap();
            assert_eq!(got, sample_bilinear(level, 0.31, 0.67));
        }
    }

    #[test]
    fn trilinear_blends_adjacent_levels() {
        let d = doc();
        let levels = d.levels().unwrap();
        assert!(levels.len() >= 2, "需要至少两级 mip");
        let a = sample_bilinear(&levels[0], 0.4, 0.6);
        let b = sample_bilinear(&levels[1], 0.4, 0.6);
        let got = d.sample_trilinear(0.4, 0.6, 0.5).unwrap();
        for c in 0..3 {
            let want = ((a[c] as f32 + b[c] as f32) / 2.0).round() as u8;
            assert_eq!(got[c], want, "通道 {c} 应为两级均值");
        }
    }
}

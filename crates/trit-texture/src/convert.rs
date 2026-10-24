//! TTX ↔ 像素/容器之间的**转换胶水**（纯函数）。
//!
//! 本模块只做「已有编解码 + 已有容器」的串联，不引入任何外部解码依赖；
//! PNG 等文件格式 I/O 在 `trit-convert` 实验层（docs/TTX.md §7 的分层规矩）。

use crate::ttx::{self as v1};
use crate::ttx2::{self as v2, TtxDocument};
use crate::{TextureError, TritTexture};

/// 一张 RGBA 图（8 位/通道）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbaImage {
    /// 宽（像素）。
    pub width: usize,
    /// 高（像素）。
    pub height: usize,
    /// RGBA 字节，`width × height × 4`，行主序。
    pub rgba: Vec<u8>,
}

fn texture_to_image(texture: &TritTexture) -> Result<RgbaImage, TextureError> {
    Ok(RgbaImage {
        width: texture.width,
        height: texture.height,
        rgba: crate::decode_rgba(texture)?,
    })
}

/// RGBA → TTX1 容器（单级纹理）。
pub fn rgba_to_ttx1(pixels: &[u8], width: usize, height: usize) -> Result<Vec<u8>, TextureError> {
    Ok(v1::to_ttx(&crate::encode_rgba(pixels, width, height)?))
}

/// TTX1 容器 → RGBA（alpha 恒 255）。
pub fn ttx1_to_rgba(bytes: &[u8]) -> Result<RgbaImage, TextureError> {
    texture_to_image(&v1::from_ttx(bytes)?)
}

/// RGBA → TTX2 容器（kind=TEX，单级）。
pub fn rgba_to_ttx2(pixels: &[u8], width: usize, height: usize) -> Result<Vec<u8>, TextureError> {
    let texture = crate::encode_rgba(pixels, width, height)?;
    v2::to_ttx2(&TtxDocument::Texture {
        levels: vec![texture],
    })
}

/// TTX2（kind=TEX）→ 最细一级（level 0）的 RGBA；kind=MOD 是结构化错误（模型不是图）。
pub fn ttx2_to_rgba(bytes: &[u8]) -> Result<RgbaImage, TextureError> {
    let TtxDocument::Texture { levels } = v2::from_ttx2(bytes)? else {
        return Err(TextureError::TtxKind(v2::KIND_MOD));
    };
    // mip 链取第 0 级（最细）：预览/回转换都以原始分辨率说话。
    let texture = levels.first().ok_or(TextureError::TtxEmpty)?;
    texture_to_image(texture)
}

/// TTX1 → TTX2（升级：块数据原样进单级 TEX，校验尾按 v2 的 CRC-16 规则重算）。
pub fn ttx1_to_ttx2(bytes: &[u8]) -> Result<Vec<u8>, TextureError> {
    let texture = v1::from_ttx(bytes)?;
    v2::to_ttx2(&TtxDocument::Texture {
        levels: vec![texture],
    })
}

/// TTX2 → TTX1（降级）：**仅单级 TEX**——mip 链降级会丢级，必须显式拒绝。
pub fn ttx2_to_ttx1(bytes: &[u8]) -> Result<Vec<u8>, TextureError> {
    let TtxDocument::Texture { levels } = v2::from_ttx2(bytes)? else {
        return Err(TextureError::TtxKind(v2::KIND_MOD));
    };
    if levels.len() != 1 {
        return Err(TextureError::TtxMip { level: 1 });
    }
    Ok(v1::to_ttx(&levels[0]))
}

/// 把下一级尺寸向下取到块对齐（宽 %4、高 %10），不能再严格递减时返回 `None`。
fn next_level_size(width: usize, height: usize) -> Option<(usize, usize)> {
    let w = (width / 2).div_ceil(crate::BLOCK_WIDTH) * crate::BLOCK_WIDTH;
    let h = (height / 2).div_ceil(crate::BLOCK_HEIGHT) * crate::BLOCK_HEIGHT;
    (w < width && h < height).then_some((w, h))
}

/// 2×2 盒式滤波降采样（RGB 平均、alpha 恒 255）；源尺寸为奇数时多余行列不参与。
fn downsample(src: &RgbaImage, width: usize, height: usize) -> RgbaImage {
    let mut rgba = vec![0u8; width * height * 4];
    for y in 0..height {
        for x in 0..width {
            let mut sum = [0u32; 3];
            for (dy, dx) in [(0usize, 0usize), (0, 1), (1, 0), (1, 1)] {
                let sx = (x * 2 + dx).min(src.width - 1);
                let sy = (y * 2 + dy).min(src.height - 1);
                let offset = (sy * src.width + sx) * 4;
                for (channel, acc) in sum.iter_mut().enumerate() {
                    *acc += u32::from(src.rgba[offset + channel]);
                }
            }
            let offset = (y * width + x) * 4;
            for (channel, acc) in sum.iter().enumerate() {
                rgba[offset + channel] = (acc / 4) as u8;
            }
            rgba[offset + 3] = 255;
        }
    }
    RgbaImage {
        width,
        height,
        rgba,
    }
}

/// 从原图构建 TT3 mip 链（0 级 = 原图，逐级 2×2 盒式降采样，尺寸向下取到
/// 块对齐，不能再严格递减即停）。返回的链可直接进 TTX2 容器。
pub fn build_mip_chain(
    pixels: &[u8],
    width: usize,
    height: usize,
) -> Result<Vec<TritTexture>, TextureError> {
    let mut levels = vec![crate::encode_rgba(pixels, width, height)?];
    let mut current = RgbaImage {
        width,
        height,
        rgba: pixels.to_vec(),
    };
    while let Some((w, h)) = next_level_size(current.width, current.height) {
        current = downsample(&current, w, h);
        levels.push(crate::encode_rgba(&current.rgba, w, h)?);
    }
    Ok(levels)
}

/// RGBA → TTX2（kind=TEX + mip 链）。
pub fn rgba_to_ttx2_mips(
    pixels: &[u8],
    width: usize,
    height: usize,
) -> Result<Vec<u8>, TextureError> {
    v2::to_ttx2(&TtxDocument::Texture {
        levels: build_mip_chain(pixels, width, height)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(width: usize, height: usize) -> Vec<u8> {
        (0..width * height * 4).map(|i| (i % 251) as u8).collect()
    }

    #[test]
    fn rgba_roundtrips_through_both_containers() {
        let pixels = sample(8, 20);
        // 参考值：直接编解码（不过容器）。
        let reference = crate::decode_rgba(&crate::encode_rgba(&pixels, 8, 20).unwrap()).unwrap();
        let v1_bytes = rgba_to_ttx1(&pixels, 8, 20).unwrap();
        let v2_bytes = rgba_to_ttx2(&pixels, 8, 20).unwrap();
        assert_eq!(ttx1_to_rgba(&v1_bytes).unwrap().rgba, reference);
        assert_eq!(ttx2_to_rgba(&v2_bytes).unwrap().rgba, reference);
        for (bytes, w, h) in [(v1_bytes, 8, 20), (v2_bytes, 8, 20)] {
            let back = if bytes[..4] == *b"TTX1" {
                ttx1_to_rgba(&bytes).unwrap()
            } else {
                ttx2_to_rgba(&bytes).unwrap()
            };
            assert_eq!((back.width, back.height), (w, h));
        }
    }

    #[test]
    fn containers_upgrade_and_downgrade_losslessly() {
        let pixels = sample(8, 20);
        let v1_bytes = rgba_to_ttx1(&pixels, 8, 20).unwrap();
        let upgraded = ttx1_to_ttx2(&v1_bytes).unwrap();
        assert_eq!(&upgraded[..4], &v2::MAGIC);
        // 升级不碰块数据：解码结果与直读 v1 逐字节一致。
        assert_eq!(
            ttx2_to_rgba(&upgraded).unwrap(),
            ttx1_to_rgba(&v1_bytes).unwrap()
        );
        // 降级回到逐字节相同的 v1 文件（头部/校验尾按 v1 规则重算）。
        let downgraded = ttx2_to_ttx1(&upgraded).unwrap();
        assert_eq!(downgraded, v1_bytes);
    }

    #[test]
    fn downgrade_rejects_mip_chains_and_model_kind() {
        let pixels = sample(8, 20);
        let mips = rgba_to_ttx2_mips(&pixels, 8, 20).unwrap();
        // 8×20 的下一级 4×10 仍对齐 ⇒ 至少两级；降级必须拒绝。
        assert!(matches!(
            ttx2_to_ttx1(&mips),
            Err(TextureError::TtxMip { level: 1 })
        ));
        // 取级钉死：mip 链解码取**第 0 级**（8×20），不是末级。
        let level0 = ttx2_to_rgba(&mips).unwrap();
        assert_eq!((level0.width, level0.height), (8, 20));
        // MOD 不是图：ttx2_to_rgba 显式拒绝。
        use crate::ttx2::{TritModel, TritModelLayer};
        let layer = TritModelLayer::new(&[vec![1, -1, 0]], &[0]).unwrap();
        let model_bytes =
            v2::to_ttx2(&TtxDocument::Model(TritModel::new(vec![layer]).unwrap())).unwrap();
        assert!(matches!(
            ttx2_to_rgba(&model_bytes),
            Err(TextureError::TtxKind(v2::KIND_MOD))
        ));
    }

    #[test]
    fn mip_chain_dimensions_and_box_filter_are_exact() {
        let mut pixels = sample(8, 20);
        pixels[0] = 255;
        pixels[1] = 255;
        pixels[2] = 255;
        pixels[4] = 0;
        pixels[5] = 100;
        pixels[6] = 200;
        let image = RgbaImage {
            width: 8,
            height: 20,
            rgba: pixels.clone(),
        };
        // 滤波器直测（不经有损的 TT3 编码）：粗像素 = 源左上 2×2 逐通道平均。
        let coarse = downsample(&image, 4, 10);
        assert_eq!((coarse.width, coarse.height), (4, 10));
        let at = |x: usize, y: usize| {
            let o = (y * 8 + x) * 4;
            [pixels[o], pixels[o + 1], pixels[o + 2]]
        };
        for channel in 0..3 {
            let expected = (u32::from(at(0, 0)[channel])
                + u32::from(at(1, 0)[channel])
                + u32::from(at(0, 1)[channel])
                + u32::from(at(1, 1)[channel]))
                / 4;
            assert_eq!(
                u32::from(coarse.rgba[channel]),
                expected,
                "channel {channel}"
            );
            assert_eq!(coarse.rgba[3], 255, "alpha 恒 255");
        }
        // 8×20 → 4×10 →（2 或 5 不再对齐）停：恰好两级。
        let levels = build_mip_chain(&pixels, 8, 20).unwrap();
        assert_eq!(
            levels
                .iter()
                .map(|t| (t.width, t.height))
                .collect::<Vec<_>>(),
            vec![(8, 20), (4, 10)]
        );
    }
}

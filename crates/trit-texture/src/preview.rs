//! 最小渲染器：把 TTX 纹理按 UV 网格画成一张 RGBA 图（给 CLI 的 `render` 子命令用）。
//!
//! 存在的意义是**可视化对拍**：同一张纹理用不同采样口径（最近邻 / 双线性 / 三线性 /
//! 自动 LOD）渲出来，肉眼就能看出缩小时的走样与 mip 的作用。逻辑放库里，
//! 不塞进 bin——这样默认测试就能覆盖，不用开 `png` feature。

use crate::sample::{Sampler, Wrap, lod_from_derivatives};
use crate::ttx2::TtxDocument;
use crate::{BLOCK_HEIGHT, BLOCK_WIDTH, TRIT_BLOCK_BYTES, TRIT_PAYLOAD_BYTES, TRITS_PER_BLOCK};

/// 采样口径。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SampleMode {
    /// 最近邻（只取第 0 级）——缩小一定走样，用来当对照。
    Nearest,
    /// 双线性（只取第 0 级）。
    Bilinear,
    /// 显式 LOD 的三线性。
    Trilinear(f32),
    /// 由屏幕空间导数自动选 LOD（光栅化器的默认）。
    Auto,
}

/// 渲染参数。
#[derive(Clone, Copy, Debug)]
pub struct RenderOptions {
    /// 输出宽（像素）。
    pub width: usize,
    /// 输出高（像素）。
    pub height: usize,
    /// UV 缩放：1.0 = 整张纹理铺满；>1 = 平铺（配合 [`Wrap::Repeat`]）。
    pub tiles: f32,
    /// 采样口径。
    pub mode: SampleMode,
    /// 越界寻址。
    pub wrap: Wrap,
    /// 左半固定用最近邻、右半用 `mode`（并排对拍）。
    pub split: bool,
    /// 只用第 `level` 级（越界夹到最后一集）。
    pub level: usize,
    /// 叠加块边界网格（洋红），用来核对块尺寸/对齐。
    pub overlay_blocks: bool,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            width: 256,
            height: 256,
            tiles: 1.0,
            mode: SampleMode::Auto,
            wrap: Wrap::Clamp,
            split: false,
            level: 0,
            overlay_blocks: false,
        }
    }
}

/// 渲染结果：(宽, 高, RGBA 字节)。
pub fn render(doc: &TtxDocument, opts: &RenderOptions) -> Result<(usize, usize, Vec<u8>), String> {
    let levels = doc.levels().ok_or("不是纹理文档（kind=MOD），无法渲染")?;
    if levels.is_empty() {
        return Err("纹理没有 mip 级".into());
    }
    let level = opts.level.min(levels.len() - 1);
    let active = &levels[level];
    if opts.width == 0 || opts.height == 0 {
        return Err("输出宽高必须 > 0".into());
    }
    let (tiles, w, h) = (opts.tiles, opts.width, opts.height);
    // 屏幕空间导数：一个输出像素跨多少 uv
    let du = tiles / w as f32;
    let dv = tiles / h as f32;
    let mut out = vec![0u8; w * h * 4];
    let mut samplers: Vec<Sampler> = levels
        .iter()
        .map(|tex| Sampler::with_wrap(tex, opts.wrap))
        .collect();
    for y in 0..h {
        let v = (y as f32 + 0.5) / h as f32 * tiles;
        for x in 0..w {
            let u = (x as f32 + 0.5) / w as f32 * tiles;
            let use_nearest = opts.mode == SampleMode::Nearest || (opts.split && x * 2 < w);
            let mut pixel = if use_nearest {
                let px = (u * active.width as f32) as i64;
                let py = (v * active.height as f32) as i64;
                samplers[level].texel(px.max(0) as usize, py.max(0) as usize)
            } else {
                match opts.mode {
                    SampleMode::Nearest => unreachable!(),
                    SampleMode::Bilinear => samplers[level].sample_bilinear(u, v),
                    SampleMode::Trilinear(lod) => {
                        trilinear(&mut samplers, u, v, lod).unwrap_or([0, 0, 0, 255])
                    }
                    SampleMode::Auto => {
                        let lod =
                            lod_from_derivatives(active.width, active.height, du, 0.0, 0.0, dv);
                        trilinear(&mut samplers, u, v, lod).unwrap_or([0, 0, 0, 255])
                    }
                }
            };
            if opts.overlay_blocks {
                let tx =
                    ((u * active.width as f32) as i64).rem_euclid(active.width as i64) as usize;
                let ty =
                    ((v * active.height as f32) as i64).rem_euclid(active.height as i64) as usize;
                if tx.is_multiple_of(BLOCK_WIDTH) || ty.is_multiple_of(BLOCK_HEIGHT) {
                    // 半透明洋红：边界要看清，但不能把内容压掉
                    for (c, target) in [255u16, 0, 255].iter().enumerate() {
                        pixel[c] = ((pixel[c] as u16 + target) / 2) as u8;
                    }
                }
            }
            let offset = (y * w + x) * 4;
            out[offset..offset + 4].copy_from_slice(&pixel);
        }
    }
    Ok((w, h, out))
}

/// 有状态采样器的三线性（复用各级的块缓存）。
fn trilinear(samplers: &mut [Sampler], u: f32, v: f32, lod: f32) -> Option<[u8; 4]> {
    if samplers.is_empty() {
        return None;
    }
    let last = (samplers.len() - 1) as f32;
    let lod = if lod.is_nan() {
        0.0
    } else {
        lod.clamp(0.0, last)
    };
    let l0 = lod.floor() as usize;
    let l1 = (l0 + 1).min(samplers.len() - 1);
    let t = lod - l0 as f32;
    let a = samplers[l0].sample_bilinear(u, v);
    if l1 == l0 || t <= 0.0 {
        return Some(a);
    }
    let b = samplers[l1].sample_bilinear(u, v);
    let mut out = [255u8; 4];
    for (c, slot) in out.iter_mut().enumerate().take(3) {
        *slot = (a[c] as f32 * (1.0 - t) + b[c] as f32 * t)
            .round()
            .clamp(0.0, 255.0) as u8;
    }
    Some(out)
}

/// 一级 mip 的账单。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LevelStat {
    /// 级下标（0 = 最细）。
    pub level: usize,
    /// 宽（像素）。
    pub width: usize,
    /// 高（像素）。
    pub height: usize,
    /// 块数。
    pub blocks: usize,
    /// 该级块流字节数。
    pub bytes: usize,
    /// 同分辨率 RGBA 字节数。
    pub rgba_bytes: usize,
    /// 载荷里实际用掉的 trit 槽 / 总槽（基-3 打包粒度税）。
    pub payload_used: usize,
    /// 载荷总槽数。
    pub payload_slots: usize,
}

/// 逐级统计（不渲染，纯账单）。
pub fn level_stats(doc: &TtxDocument) -> Result<Vec<LevelStat>, String> {
    let levels = doc.levels().ok_or("不是纹理文档（kind=MOD），无 mip 级")?;
    let mut out = Vec::with_capacity(levels.len());
    for (level, tex) in levels.iter().enumerate() {
        let blocks = tex.blocks.len();
        out.push(LevelStat {
            level,
            width: tex.width,
            height: tex.height,
            blocks,
            bytes: blocks * TRIT_BLOCK_BYTES,
            rgba_bytes: tex.width * tex.height * 4,
            payload_used: TRITS_PER_BLOCK,
            payload_slots: TRIT_PAYLOAD_BYTES * 5,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::convert::rgba_to_ttx2_mips;
    use crate::ttx2::from_ttx2;

    fn solid(width: usize, height: usize, rgb: [u8; 3]) -> TtxDocument {
        let mut px = vec![0u8; width * height * 4];
        for chunk in px.as_chunks_mut::<4>().0 {
            chunk[..3].copy_from_slice(&rgb);
            chunk[3] = 255;
        }
        let bytes = rgba_to_ttx2_mips(&px, width, height).expect("encode");
        from_ttx2(&bytes).expect("parse")
    }

    #[test]
    fn solid_texture_renders_solid_in_every_mode() {
        let doc = solid(16, 20, [90, 140, 200]);
        // 期望值要取**解码后**的颜色：RGB565 端点会量化（90,140,200 → 90,142,206）
        let want = {
            let tex = &doc.levels().unwrap()[0];
            crate::sample::texel(tex, 0, 0)
        };
        for mode in [
            SampleMode::Nearest,
            SampleMode::Bilinear,
            SampleMode::Trilinear(0.0),
            SampleMode::Auto,
        ] {
            let opts = RenderOptions {
                width: 32,
                height: 32,
                tiles: 2.0,
                mode,
                wrap: Wrap::Repeat,
                ..RenderOptions::default()
            };
            let (w, h, rgba) = render(&doc, &opts).expect("render");
            assert_eq!((w, h), (32, 32));
            for (i, px) in rgba.as_chunks::<4>().0.iter().enumerate() {
                assert_eq!(
                    [px[0], px[1], px[2], px[3]],
                    want,
                    "mode={mode:?} 第 {i} 个像素应为解码后的纯色"
                );
            }
        }
    }

    #[test]
    fn split_keeps_geometry_and_alpha() {
        let doc = solid(16, 20, [10, 20, 30]);
        let opts = RenderOptions {
            width: 40,
            height: 24,
            tiles: 1.0,
            mode: SampleMode::Auto,
            wrap: Wrap::Clamp,
            split: true,
            ..RenderOptions::default()
        };
        let (w, h, rgba) = render(&doc, &opts).expect("render");
        assert_eq!(w * h * 4, rgba.len());
        assert!(rgba.as_chunks::<4>().0.iter().all(|p| p[3] == 255));
    }

    #[test]
    fn overlay_marks_only_block_boundaries() {
        let doc = solid(16, 20, [90, 140, 200]);
        let want = {
            let tex = &doc.levels().unwrap()[0];
            crate::sample::texel(tex, 1, 1)
        };
        let opts = RenderOptions {
            width: 16,
            height: 20,
            tiles: 1.0,
            mode: SampleMode::Nearest,
            wrap: Wrap::Clamp,
            split: false,
            level: 0,
            overlay_blocks: true,
        };
        let (_, _, rgba) = render(&doc, &opts).expect("render");
        let pixels: Vec<[u8; 4]> = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| [p[0], p[1], p[2], p[3]])
            .collect();
        // 1:1 渲染时，(1,1) 不在块边界上 ⇒ 原样
        assert_eq!(pixels[20 + 1], want);
        // (0,0) 在块边界上 ⇒ 与洋红半混：R/B 抬升、G 压低
        let corner = pixels[0];
        assert!(
            corner[0] > corner[1] && corner[2] > corner[1],
            "边界应为品红偏色：{corner:?}"
        );
        assert_ne!(corner, want);
        assert!(pixels.contains(&want), "块内部的像素不应被覆盖");
    }

    #[test]
    fn level_stats_accounting() {
        let doc = solid(16, 20, [1, 2, 3]);
        let stats = level_stats(&doc).expect("stats");
        assert!(stats.len() >= 2, "mip 链应有多级");
        assert_eq!(stats[0].level, 0);
        assert_eq!((stats[0].width, stats[0].height), (16, 20));
        assert_eq!(
            stats[0].blocks,
            (16 / crate::BLOCK_WIDTH) * (20 / crate::BLOCK_HEIGHT)
        );
        assert_eq!(stats[0].bytes, stats[0].blocks * crate::TRIT_BLOCK_BYTES);
        assert_eq!(stats[0].rgba_bytes, 16 * 20 * 4);
        assert_eq!(stats[0].payload_used, crate::TRITS_PER_BLOCK);
        assert_eq!(stats[0].payload_slots, crate::TRIT_PAYLOAD_BYTES * 5);
        // 每级都在严格变小
        for pair in stats.windows(2) {
            assert!(pair[1].width <= pair[0].width && pair[1].height <= pair[0].height);
            assert!(pair[1].bytes < pair[0].bytes);
        }
    }

    #[test]
    fn rejects_model_documents() {
        let layer = crate::ttx2::TritModelLayer::new(&[vec![0i8; 4]], &[0i8]).expect("layer");
        let doc = TtxDocument::Model(crate::ttx2::TritModel::new(vec![layer]).expect("model"));
        let err = render(&doc, &RenderOptions::default()).unwrap_err();
        assert!(err.contains("kind=MOD"));
    }
}

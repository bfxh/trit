//! 压缩域纹理采样：**不整图解码**，按需从块里取纹素再做过滤。
//!
//! 引擎侧只把块流传进显存，片元着色器采样时现解码；这里是同一套语义的 CPU
//! 参考实现（也是 GPU 侧的对拍基线）。坐标约定与图形 API 一致：
//! `u, v ∈ [0, 1]`，**纹素中心**在 `(i + 0.5) / 尺寸`，越界按 **clamp-to-edge**。
//!
//! 只在 3 级调色板上取值 ⇒ 双线性必须解 4 个纹素（可能跨 2×2 个块），
//! 但不需要把整张图展开成 RGBA：一次采样最多碰 4 个块。块形状**不写死**，
//! 统一取 `BLOCK_WIDTH` / `BLOCK_HEIGHT`（当前 4×10，见 lib.rs）。
//!
//! ## 实测（本机，`cargo bench -p trit-texture --bench sample`，光栅顺序，5 轮中位数）
//!
//! | 路径 | 512×520 | 2048×2040 |
//! |---|---:|---:|
//! | 最近邻（压缩域） | 12.8 ns | 12.8 ns |
//! | 双线性（压缩域按需解） | 69.6 ns | 67.8 ns |
//! | 双线性（`Sampler` + 块缓存） | 59.8 ns | 58.2 ns |
//! | 双线性（整图 RGBA） | 32.7 ns | 29.9 ns |
//!
//! ## 光栅化器需要的那一对
//!
//! - **越界寻址**：[`Wrap::Clamp`] / [`Wrap::Repeat`] / [`Wrap::Mirror`]，
//!   对 `Sampler` 与自由函数都生效；索引解析是精确的整数运算（`wrap_index`）。
//! - **自动 LOD**：[`lod_from_derivatives`] 按 `ρ = max(|∂/∂x|, |∂/∂y|)` 求 `log2`，
//!   [`TtxDocument::sample_auto`] 直接吃 `dpdx`/`dpdy` 风格的四元导数。
//!
//! **结论（如实记）：CPU 上压缩域双线性比「整图解码一次再过滤」慢约 2×**，
//! 换来 **13.3× 的常驻内存**（2048×2040：1.2 MB vs 16 MB）。所以：
//! - 这个模块在 CPU 侧是**参考实现 / GPU 对拍基线**，不是快路径；
//! - 真正的收益在 GPU——那里瓶颈是带宽，RGBA 路径要多搬 13× 的数据；
//! - 别把「压缩域采样更快」写进宣传：CPU 上不成立，实测会打脸。

use crate::ttx2::TtxDocument;
use crate::{BLOCK_HEIGHT, BLOCK_WIDTH, TritTexture, palette_from_endpoints};

/// 越界寻址：与图形 API 的三档对齐。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Wrap {
    /// 夹到边缘纹素（默认）。
    #[default]
    Clamp,
    /// 平铺重复。
    Repeat,
    /// 镜像重复（`GL_MIRRORED_REPEAT`，边缘不重复）。
    Mirror,
}

/// 图集里的子矩形（纹素坐标：左上角 + 宽高）。
///
/// 采样器带矩形时，**UV 的 `[0,1]` 映射到矩形内**，`wrap` 也只在矩形内循环——
/// 否则边界双线性会采到相邻图集条目（经典 bleed）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    /// 左边界（纹素）。
    pub x: usize,
    /// 上边界（纹素）。
    pub y: usize,
    /// 宽（纹素，> 0）。
    pub width: usize,
    /// 高（纹素，> 0）。
    pub height: usize,
}

impl Rect {
    /// 构造；宽或高为 0 返回 `None`。
    pub fn new(x: usize, y: usize, width: usize, height: usize) -> Option<Self> {
        (width > 0 && height > 0).then_some(Self {
            x,
            y,
            width,
            height,
        })
    }

    /// 取第 `level` 级 mip 上对应的子矩形（每级尺寸减半，至少 1×1）。
    pub fn at_level(&self, level: u32) -> Self {
        Self {
            x: self.x >> level,
            y: self.y >> level,
            width: (self.width >> level).max(1),
            height: (self.height >> level).max(1),
        }
    }
}

/// 把越界下标按 `wrap` 解析成 `[0, n)`。`n` 必须 > 0。
#[inline]
fn wrap_index(i: i64, n: i64, wrap: Wrap) -> usize {
    match wrap {
        Wrap::Clamp => i.clamp(0, n - 1) as usize,
        Wrap::Repeat => i.rem_euclid(n) as usize,
        Wrap::Mirror => {
            let period = 2 * n;
            let r = i.rem_euclid(period);
            if r < n {
                r as usize
            } else {
                (period - 1 - r) as usize
            }
        }
    }
}

/// 3^k 查表：5 trit/字节是 `Σ digit·3^k`，取第 k 位就是 `(byte / 3^k) % 3`。
const POW3: [u32; 5] = [1, 3, 9, 27, 81];

/// 取一个纹素的 RGBA（最近邻，越界 clamp-to-edge）。TT3 不存 alpha，恒 255。
///
/// **只解这一个 trit**：整块 `unpack_trits` 是 40 次除法，采样热路径付不起。
pub fn texel(texture: &TritTexture, x: usize, y: usize) -> [u8; 4] {
    let x = x.min(texture.width.saturating_sub(1));
    let y = y.min(texture.height.saturating_sub(1));
    let block_cols = texture.width / BLOCK_WIDTH;
    let block_index = (y / BLOCK_HEIGHT) * block_cols + (x / BLOCK_WIDTH);
    let block = &texture.blocks[block_index];
    let e0 = u16::from_be_bytes([block[0], block[1]]);
    let e1 = u16::from_be_bytes([block[2], block[3]]);
    let palette = palette_from_endpoints(e0, e1);
    let local = (y % BLOCK_HEIGHT) * BLOCK_WIDTH + (x % BLOCK_WIDTH);
    let byte = block[4 + local / 5] as u32;
    let slot = (byte / POW3[local % 5]) % 3;
    let (r, g, b) = palette[slot as usize];
    [r, g, b, 255]
}

/// 双线性采样（`u, v ∈ [0,1]`，纹素中心约定，clamp-to-edge）。
///
/// 返回值与 [`crate::decode_rgba`] 同一色彩空间（8 位 RGB，alpha 恒 255）；
/// 权重在 f32 里算完再四舍五入回 8 位。
pub fn sample_bilinear(texture: &TritTexture, u: f32, v: f32) -> [u8; 4] {
    sample_bilinear_with(texture, u, v, Wrap::Clamp)
}

/// 双线性采样，越界按 `wrap` 解析（一次性调用用这个；循环里用 [`Sampler`] 更省）。
pub fn sample_bilinear_with(texture: &TritTexture, u: f32, v: f32, wrap: Wrap) -> [u8; 4] {
    Sampler::with_wrap(texture, wrap).sample_bilinear(u, v)
}

/// 有状态的采样器：带一块**块调色板缓存**。
///
/// 光栅扫描下同一块会被连采 `BLOCK_PIXELS` 次（4×10 = 40 次），每次重解端点+调色板
/// 是纯浪费；直接映射 4 槽缓存把「每样点 4 次解码」压到「每块一次」。
pub struct Sampler<'a> {
    texture: &'a TritTexture,
    cache: [CacheSlot; 4],
    wrap: Wrap,
    rect: Option<Rect>,
}

/// (块下标, 该块的 3 级调色板)。
type CacheSlot = (usize, [(u8, u8, u8); 3]);

impl<'a> Sampler<'a> {
    /// 绑定一张纹理（默认 [`Wrap::Clamp`]）。
    pub fn new(texture: &'a TritTexture) -> Self {
        Self::with_wrap(texture, Wrap::Clamp)
    }

    /// 绑定一张纹理并指定越界寻址。
    pub fn with_wrap(texture: &'a TritTexture, wrap: Wrap) -> Self {
        Self {
            texture,
            cache: [(usize::MAX, [(0, 0, 0); 3]); 4],
            wrap,
            rect: None,
        }
    }

    /// 绑定图集子矩形（`None` = 整张纹理）。越界的矩形会被夹到纹理内。
    pub fn set_rect(&mut self, rect: Option<Rect>) {
        self.rect = rect.map(|r| {
            let x = r.x.min(self.texture.width.saturating_sub(1));
            let y = r.y.min(self.texture.height.saturating_sub(1));
            Rect {
                x,
                y,
                width: r.width.min(self.texture.width - x).max(1),
                height: r.height.min(self.texture.height - y).max(1),
            }
        });
    }

    /// 当前子矩形（`None` = 整张）。
    pub fn rect(&self) -> Option<Rect> {
        self.rect
    }

    /// 采样域：`(x0, y0, w, h)`，无矩形时就是整张纹理。
    #[inline]
    fn bounds(&self) -> (usize, usize, usize, usize) {
        match self.rect {
            Some(r) => (r.x, r.y, r.width, r.height),
            None => (0, 0, self.texture.width, self.texture.height),
        }
    }

    /// 当前越界寻址。
    pub fn wrap(&self) -> Wrap {
        self.wrap
    }

    /// 切换越界寻址（缓存不清：缓存键是块下标，与寻址无关）。
    pub fn set_wrap(&mut self, wrap: Wrap) {
        self.wrap = wrap;
    }

    #[inline]
    fn palette(&mut self, block_index: usize) -> [(u8, u8, u8); 3] {
        let slot = block_index & 3;
        if self.cache[slot].0 == block_index {
            return self.cache[slot].1;
        }
        let block = &self.texture.blocks[block_index];
        let e0 = u16::from_be_bytes([block[0], block[1]]);
        let e1 = u16::from_be_bytes([block[2], block[3]]);
        let palette = palette_from_endpoints(e0, e1);
        self.cache[slot] = (block_index, palette);
        palette
    }

    /// 最近邻取纹素（越界按 [`Sampler::wrap`] 解析）。
    pub fn texel(&mut self, x: usize, y: usize) -> [u8; 4] {
        self.texel_at(x as i64, y as i64)
    }

    #[inline]
    fn texel_at(&mut self, xi: i64, yi: i64) -> [u8; 4] {
        let tex = self.texture;
        let (x0, y0, w, h) = self.bounds();
        let x = x0 + wrap_index(xi, w as i64, self.wrap);
        let y = y0 + wrap_index(yi, h as i64, self.wrap);
        let block_cols = tex.width / BLOCK_WIDTH;
        let block_index = (y / BLOCK_HEIGHT) * block_cols + (x / BLOCK_WIDTH);
        let local = (y % BLOCK_HEIGHT) * BLOCK_WIDTH + (x % BLOCK_WIDTH);
        let byte = tex.blocks[block_index][4 + local / 5] as u32;
        let slot = (byte / POW3[local % 5]) % 3;
        let (r, g, b) = self.palette(block_index)[slot as usize];
        [r, g, b, 255]
    }

    /// 双线性采样（语义与 [`sample_bilinear`] 完全一致，只是复用调色板缓存）。
    pub fn sample_bilinear(&mut self, u: f32, v: f32) -> [u8; 4] {
        let tex = self.texture;
        if tex.width == 0 || tex.height == 0 || tex.blocks.is_empty() {
            return [0, 0, 0, 255];
        }
        let (u, v) = match self.wrap {
            Wrap::Clamp => (u.clamp(0.0, 1.0), v.clamp(0.0, 1.0)),
            _ => (u, v),
        };
        let (_, _, w, h) = self.bounds();
        let fx = u * w as f32 - 0.5;
        let fy = v * h as f32 - 0.5;
        let x0 = fx.floor();
        let y0 = fy.floor();
        let tx = fx - x0;
        let ty = fy - y0;
        let (x0i, y0i) = (x0 as i64, y0 as i64);
        let p00 = self.texel_at(x0i, y0i);
        let p10 = self.texel_at(x0i + 1, y0i);
        let p01 = self.texel_at(x0i, y0i + 1);
        let p11 = self.texel_at(x0i + 1, y0i + 1);
        let mut out = [0u8; 4];
        for c in 0..3 {
            let top = p00[c] as f32 * (1.0 - tx) + p10[c] as f32 * tx;
            let bottom = p01[c] as f32 * (1.0 - tx) + p11[c] as f32 * tx;
            out[c] = (top * (1.0 - ty) + bottom * ty).round().clamp(0.0, 255.0) as u8;
        }
        out[3] = 255;
        out
    }
}

/// 由屏幕空间导数推 LOD（各向同性近似，与 GPU 的 `ρ = max(|∂/∂x|, |∂/∂y|)` 同式）。
///
/// `du_x`/`dv_x` 是 uv 对屏幕 x 的偏导，`du_y`/`dv_y` 对屏幕 y；返回未夹取的
/// `log2(ρ)`（ρ ≤ 1 时返回 0，放大不改 LOD）。
pub fn lod_from_derivatives(
    width: usize,
    height: usize,
    du_x: f32,
    dv_x: f32,
    du_y: f32,
    dv_y: f32,
) -> f32 {
    let w = width as f32;
    let h = height as f32;
    let dx = ((du_x * w) * (du_x * w) + (dv_x * h) * (dv_x * h)).sqrt();
    let dy = ((du_y * w) * (du_y * w) + (dv_y * h) * (dv_y * h)).sqrt();
    let rho = dx.max(dy);
    if !rho.is_finite() || rho <= 0.0 {
        0.0
    } else {
        rho.log2().max(0.0)
    }
}

impl TtxDocument {
    /// 按屏幕空间导数自动选 LOD 并三线性采样（光栅化器的默认取法）。
    ///
    /// 导数由调用方从顶点/像素差分算好（GPU 里是 `dpdx`/`dpdy`）；本函数只做
    /// `lod = log2(max(ρ_x, ρ_y))` 然后转给 [`TtxDocument::sample_trilinear`]。
    pub fn sample_auto(
        &self,
        u: f32,
        v: f32,
        du_x: f32,
        dv_x: f32,
        du_y: f32,
        dv_y: f32,
    ) -> Option<[u8; 4]> {
        let levels = self.levels()?;
        let base = levels.first()?;
        let lod = lod_from_derivatives(base.width, base.height, du_x, dv_x, du_y, dv_y);
        self.sample_trilinear(u, v, lod)
    }

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
        self.sample_trilinear_with(u, v, lod, Wrap::Clamp)
    }

    /// 三线性 + 指定越界寻址。
    pub fn sample_trilinear_with(&self, u: f32, v: f32, lod: f32, wrap: Wrap) -> Option<[u8; 4]> {
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
        let a = sample_bilinear_with(&levels[l0], u, v, wrap);
        if l1 == l0 || t <= 0.0 {
            return Some(a);
        }
        let b = sample_bilinear_with(&levels[l1], u, v, wrap);
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
    fn wrap_index_rules() {
        // 索引解析是精确的，先钉这一层
        assert_eq!(wrap_index(-1, 8, Wrap::Clamp), 0);
        assert_eq!(wrap_index(8, 8, Wrap::Clamp), 7);
        assert_eq!(wrap_index(-1, 8, Wrap::Repeat), 7);
        assert_eq!(wrap_index(8, 8, Wrap::Repeat), 0);
        // mirror：边缘不重复（-1 映到 0，8 映到 7，-2 映到 1）
        assert_eq!(wrap_index(-1, 8, Wrap::Mirror), 0);
        assert_eq!(wrap_index(8, 8, Wrap::Mirror), 7);
        assert_eq!(wrap_index(-2, 8, Wrap::Mirror), 1);
        assert_eq!(wrap_index(9, 8, Wrap::Mirror), 6);
    }

    #[test]
    fn wrap_repeat_tiles_the_seam() {
        let d = doc();
        let tex = &d.levels().unwrap()[0];
        // 跨 seam 的中点应≈最右列与最左列的平均（u 的浮点在 seam 处不精确，留 ±1）
        let left = texel(tex, 0, 3);
        let right = texel(tex, tex.width - 1, 3);
        // u = 1.0 正好落在最后一列与第 0 列之间（fx = W - 0.5，tx = 0.5）
        let seam = sample_bilinear_with(tex, 1.0, 3.5 / tex.height as f32, Wrap::Repeat);
        for c in 0..3 {
            let want = ((left[c] as f32 + right[c] as f32) / 2.0).round() as i32;
            assert!(
                (seam[c] as i32 - want).abs() <= 1,
                "seam 处通道 {c}: 得到 {}，期望 ≈ {want}",
                seam[c]
            );
        }
    }

    #[test]
    fn wrap_mirror_reflects_without_duplicating_edge() {
        let d = doc();
        let tex = &d.levels().unwrap()[0];
        let edge = texel(tex, 0, 2);
        // u 略小于 0：镜像后仍是最左列（不是最右列）
        let a = sample_bilinear_with(
            tex,
            -0.5 / tex.width as f32,
            2.5 / tex.height as f32,
            Wrap::Mirror,
        );
        assert_eq!(a, edge, "mirror 下 u<0 应映到第 0 列");
        // repeat 在同一位置应映到最右列
        let b = sample_bilinear_with(
            tex,
            -0.5 / tex.width as f32,
            2.5 / tex.height as f32,
            Wrap::Repeat,
        );
        assert_eq!(
            b,
            texel(tex, tex.width - 1, 2),
            "repeat 下 u<0 应映到最后一列"
        );
    }

    #[test]
    fn lod_from_derivatives_basics() {
        let (w, h) = (64usize, 40usize);
        // 一个像素走一个纹素 ⇒ lod 0
        assert_eq!(
            lod_from_derivatives(w, h, 1.0 / w as f32, 0.0, 0.0, 1.0 / h as f32),
            0.0
        );
        // 一个像素走 8 个纹素 ⇒ lod 3
        let lod = lod_from_derivatives(w, h, 8.0 / w as f32, 0.0, 0.0, 8.0 / h as f32);
        assert!((lod - 3.0).abs() < 1e-5, "lod={lod}");
        // 放大（导数 0）不出现负 LOD
        assert_eq!(lod_from_derivatives(w, h, 0.0, 0.0, 0.0, 0.0), 0.0);
        // 只有 y 方向变化也参与 ρ
        let lod_y = lod_from_derivatives(w, h, 0.0, 0.0, 0.0, 16.0 / h as f32);
        assert!((lod_y - 4.0).abs() < 1e-5, "lod_y={lod_y}");
    }

    #[test]
    fn sample_auto_matches_trilinear_at_same_lod() {
        let d = doc();
        let (w, h) = {
            let t = &d.levels().unwrap()[0];
            (t.width, t.height)
        };
        let (u, v) = (0.37, 0.61);
        let (du_x, dv_y) = (2.0 / w as f32, 2.0 / h as f32);
        let auto = d.sample_auto(u, v, du_x, 0.0, 0.0, dv_y).unwrap();
        let manual = d.sample_trilinear(u, v, 1.0).unwrap();
        assert_eq!(auto, manual, "lod=1 时 auto 应与显式 lod=1 一致");
    }

    /// 左半红、右半绿的 32×40 图（每块 4 宽，块内纯色 ⇒ 编码后仍然是纯红/纯绿）。
    fn two_halves() -> TtxDocument {
        let (w, h) = (32usize, 40usize);
        let mut px = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let o = (y * w + x) * 4;
                let c = if x < 16 { [220, 30, 30] } else { [30, 200, 30] };
                px[o..o + 3].copy_from_slice(&c);
                px[o + 3] = 255;
            }
        }
        let bytes = rgba_to_ttx2_mips(&px, w, h).expect("encode");
        from_ttx2(&bytes).expect("parse")
    }

    #[test]
    fn rect_maps_uv_into_sub_rect() {
        let d = two_halves();
        let tex = &d.levels().unwrap()[0];
        let rect = Rect::new(0, 0, 16, 40).unwrap();
        let mut s = Sampler::with_wrap(tex, Wrap::Clamp);
        s.set_rect(Some(rect));
        // 纹素中心逐一对照绝对坐标
        for j in 0..rect.height {
            for i in 0..rect.width {
                let u = (i as f32 + 0.5) / rect.width as f32;
                let v = (j as f32 + 0.5) / rect.height as f32;
                assert_eq!(s.sample_bilinear(u, v), texel(tex, i, j), "({i},{j})");
            }
        }
        // 矩形外的纹素绝不该出现（全是绿）
        for v in [0.0f32, 0.5, 1.0] {
            let px = s.sample_bilinear(0.0, v);
            assert!(px[0] > 200 && px[1] < 60, "左半矩形应保持红：{px:?}");
        }
    }

    #[test]
    fn rect_clamp_does_not_bleed_into_neighbour() {
        let d = two_halves();
        let tex = &d.levels().unwrap()[0];
        let rect = Rect::new(0, 0, 16, 40).unwrap();
        let mut with_rect = Sampler::with_wrap(tex, Wrap::Clamp);
        with_rect.set_rect(Some(rect));
        let mut full = Sampler::with_wrap(tex, Wrap::Clamp);
        // 不带矩形：u=1.0 落在最右列（绿）
        let a = full.sample_bilinear(1.0, 0.5);
        assert!(a[1] > 200 && a[0] < 60, "无矩形时应采到右半绿：{a:?}");
        // 带矩形：u=1.0 仍停在矩形右边缘（红）
        let b = with_rect.sample_bilinear(1.0, 0.5);
        assert!(b[0] > 200 && b[1] < 60, "带矩形时不应跨条目 bleed：{b:?}");
    }

    #[test]
    fn rect_repeat_wraps_inside_rect() {
        let d = two_halves();
        let tex = &d.levels().unwrap()[0];
        let rect = Rect::new(0, 0, 16, 40).unwrap();
        let mut s = Sampler::with_wrap(tex, Wrap::Repeat);
        s.set_rect(Some(rect));
        // u 跨过 1.0 后在矩形内回卷，仍应是红
        let px = s.sample_bilinear(1.5, 0.5);
        assert!(px[0] > 200 && px[1] < 60, "repeat 应在矩形内回卷：{px:?}");
    }

    #[test]
    fn rect_at_level_halves() {
        let r = Rect::new(16, 20, 8, 10).unwrap();
        assert_eq!(r.at_level(0), r);
        assert_eq!(r.at_level(1), Rect::new(8, 10, 4, 5).unwrap());
        let tiny = Rect::new(1, 1, 1, 1).unwrap();
        assert_eq!(tiny.at_level(3), Rect::new(0, 0, 1, 1).unwrap());
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

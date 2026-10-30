//! 多级采样器：一条 mip 链 + **每级各一个块调色板缓存**。
//!
//! [`Sampler`] 只绑一张纹理（一级）。三线性/各向异性每次采样都要换级，若每次现建
//! `Sampler`，块缓存等于没有——`sample::sample_aniso` 就是这么实现的，实测 taps=1
//! 比有缓存的三线性贵 2.1 倍。这里把每级各绑一个 `Sampler`，缓存跨调用存活，
//! 并把图集矩形按级缩小后分别下发（自由函数不支持矩形）。
//!
//! 数值口径与 `TtxDocument::sample_trilinear_with` / `sample::sample_aniso` 完全一致：
//! 同样的 `mul_add` 结合顺序、同样的 `round_ties_even`、同样的 tap 计划。所以两者
//! 只有在传入不同的 `Rect` 时才可能不同。

use crate::TritTexture;
use crate::sample::{Rect, Sampler, Wrap, lod_from_derivatives};
use crate::ttx2::TtxDocument;

/// 各向异性的取样计划。
struct Plan {
    count: u32,
    lod: f32,
    step_u: f32,
    step_v: f32,
}

/// 与 `sample::sample_aniso` 同一套算式：`ρ_major/ρ_minor` 定 tap 数，
/// `lod = log2(ρ_minor)`，沿**屏幕 x 或 y 中较长的那条**取步长。
fn plan(
    width: usize,
    height: usize,
    du_x: f32,
    dv_x: f32,
    du_y: f32,
    dv_y: f32,
    max_taps: u32,
) -> Plan {
    let w = width as f32;
    let h = height as f32;
    let dx = (du_x * w) * (du_x * w) + (dv_x * h) * (dv_x * h);
    let dy = (du_y * w) * (du_y * w) + (dv_y * h) * (dv_y * h);
    let taps = [dx.sqrt(), dy.sqrt()];
    let major = taps[0].max(taps[1]);
    let minor = taps[0].min(taps[1]);
    let max_taps = max_taps.max(1);
    if !major.is_finite() || major <= 0.0 {
        return Plan {
            count: 1,
            lod: 0.0,
            step_u: 0.0,
            step_v: 0.0,
        };
    }
    let ratio = if minor > 0.0 {
        major / minor
    } else {
        max_taps as f32
    };
    let count = (ratio.ceil() as u32).clamp(1, max_taps);
    let lod = if minor > 0.0 {
        minor.max(1.0).log2()
    } else {
        major.log2()
    };
    let (step_u, step_v) = if taps[0] >= taps[1] {
        (du_x, dv_x)
    } else {
        (du_y, dv_y)
    };
    Plan {
        count,
        lod,
        step_u,
        step_v,
    }
}

/// 两级按 `t` 融合（与 `TtxDocument::sample_trilinear_with` 同式）。
fn blend(a: [u8; 4], b: [u8; 4], t: f32) -> [u8; 4] {
    let mut out = [0u8; 4];
    for (slot, value) in out.iter_mut().enumerate().take(3) {
        *value = (a[slot] as f32)
            .mul_add(1.0 - t, b[slot] as f32 * t)
            .round_ties_even()
            .clamp(0.0, 255.0) as u8;
    }
    out[3] = 255;
    out
}

/// mip 链采样器：三线性 / 各向异性 / 自动 LOD，带每级块缓存与图集矩形。
pub struct MipSampler<'a> {
    levels: Vec<Sampler<'a>>,
    base: (usize, usize),
    rect: Option<Rect>,
    wrap: Wrap,
}

impl<'a> MipSampler<'a> {
    /// 绑定一条 mip 链（从细到粗），默认 [`Wrap::Clamp`]。
    pub fn new(levels: &'a [TritTexture]) -> Self {
        Self::with_wrap(levels, Wrap::Clamp)
    }

    /// 绑定 mip 链并指定越界寻址。
    pub fn with_wrap(levels: &'a [TritTexture], wrap: Wrap) -> Self {
        let base = levels.first().map_or((0, 0), |t| (t.width, t.height));
        Self {
            levels: levels.iter().map(|t| Sampler::with_wrap(t, wrap)).collect(),
            base,
            rect: None,
            wrap,
        }
    }

    /// 从纹理文档取 mip 链；`kind=MOD` 返回 `None`（与 `sample_trilinear` 一致，不静默给 0）。
    pub fn from_document(doc: &'a TtxDocument) -> Option<Self> {
        doc.levels().map(Self::new)
    }

    /// 绑定的级数。
    pub fn level_count(&self) -> usize {
        self.levels.len()
    }

    /// 绑定图集子矩形（级别 0 的纹素坐标；各粗级自动 `>>level`）。
    ///
    /// 越界会被夹到纹理内，且夹取结果按级各自计算。
    pub fn set_rect(&mut self, rect: Option<Rect>) {
        self.rect = rect;
        for (level, sampler) in self.levels.iter_mut().enumerate() {
            sampler.set_rect(rect.map(|r| r.at_level(level as u32)));
        }
        if let Some(first) = self.levels.first() {
            self.rect = first.rect();
        }
    }

    /// 当前子矩形（`None` = 整张；级别 0 坐标）。
    pub fn rect(&self) -> Option<Rect> {
        self.rect
    }

    /// 当前越界寻址。
    pub fn wrap(&self) -> Wrap {
        self.wrap
    }

    /// 切换越界寻址（各级缓存不清：键是块下标，与寻址无关）。
    pub fn set_wrap(&mut self, wrap: Wrap) {
        self.wrap = wrap;
        for sampler in &mut self.levels {
            sampler.set_wrap(wrap);
        }
    }

    /// 最近邻（第 0 级；矩形与 wrap 生效）。
    pub fn sample_nearest(&mut self, u: f32, v: f32) -> Option<[u8; 4]> {
        self.levels.first_mut().map(|s| s.sample_nearest(u, v))
    }

    /// 三线性采样：相邻两级各一次双线性再融合。
    ///
    /// `lod` 夹到 `[0, 级数-1]`；`NaN` 按 0 处理；只有一级时退化为双线性。
    pub fn sample_trilinear(&mut self, u: f32, v: f32, lod: f32) -> Option<[u8; 4]> {
        if self.levels.is_empty() {
            return None;
        }
        let last = (self.levels.len() - 1) as f32;
        let lod = if lod.is_nan() {
            0.0
        } else {
            lod.clamp(0.0, last)
        };
        let l0 = lod.floor() as usize;
        let l1 = (l0 + 1).min(self.levels.len() - 1);
        let t = lod - l0 as f32;
        let a = self.levels[l0].sample_bilinear(u, v);
        if l1 == l0 || t <= 0.0 {
            return Some(a);
        }
        let b = self.levels[l1].sample_bilinear(u, v);
        Some(blend(a, b, t))
    }

    /// 一次三线性（各向同性 LOD），导数由调用方算好（GPU 里是 `dpdx`/`dpdy`）。
    pub fn sample_auto(
        &mut self,
        u: f32,
        v: f32,
        du_x: f32,
        dv_x: f32,
        du_y: f32,
        dv_y: f32,
    ) -> Option<[u8; 4]> {
        let lod = lod_from_derivatives(self.base.0, self.base.1, du_x, dv_x, du_y, dv_y);
        self.sample_trilinear(u, v, lod)
    }

    /// 各向异性采样：沿长轴叠 `max_taps` 个以内三线性 tap，最后取平均。
    ///
    /// 与 `sample::sample_aniso` 同一套判定与舍入；相对它的增益来自每级块缓存，
    /// 以及支持了图集矩形（自由函数没有 `rect` 参数）。
    #[allow(clippy::too_many_arguments)]
    pub fn sample_aniso(
        &mut self,
        u: f32,
        v: f32,
        du_x: f32,
        dv_x: f32,
        du_y: f32,
        dv_y: f32,
        max_taps: u32,
    ) -> Option<[u8; 4]> {
        if self.levels.is_empty() {
            return None;
        }
        let p = plan(self.base.0, self.base.1, du_x, dv_x, du_y, dv_y, max_taps);
        if p.count == 1 {
            return self.sample_trilinear(u, v, p.lod);
        }
        let mut acc = [0f32; 3];
        for tap in 0..p.count {
            let t = (tap as f32 + 0.5) / p.count as f32 - 0.5;
            let px = self.sample_trilinear(u + p.step_u * t, v + p.step_v * t, p.lod)?;
            for (slot, value) in acc.iter_mut().enumerate() {
                *value += px[slot] as f32;
            }
        }
        let mut out = [255u8; 4];
        for (slot, value) in out.iter_mut().enumerate().take(3) {
            *value = (acc[slot] / p.count as f32)
                .round_ties_even()
                .clamp(0.0, 255.0) as u8;
        }
        Some(out)
    }
}

//! 纹理着色：把 [`SampleMode`] 落到 `MipSampler` 上，并报告该像素实际用的 LOD。

use crate::TritTexture;
use crate::mip::MipSampler;
use crate::preview::SampleMode;
use crate::sample::{Rect, Wrap, lod_from_derivatives};

use super::{Fragment, Shaded};

/// 绑好 mip 链的纹理着色器（一个对象跨整帧复用，块缓存跟着活）。
pub(crate) struct Textured<'a> {
    mip: MipSampler<'a>,
    mode: SampleMode,
    base: (usize, usize),
}

impl<'a> Textured<'a> {
    /// `levels` 必须非空（调用方已校验）。
    pub(crate) fn new(
        levels: &'a [TritTexture],
        mode: SampleMode,
        wrap: Wrap,
        rect: Option<Rect>,
    ) -> Self {
        let base = levels.first().map_or((0, 0), |t| (t.width, t.height));
        let mut mip = MipSampler::with_wrap(levels, wrap);
        mip.set_rect(rect);
        Self { mip, mode, base }
    }

    pub(crate) fn shade(&mut self, f: &Fragment) -> Shaded {
        let [u, v] = f.uv;
        let d = f.derivative;
        let lod = match self.mode {
            SampleMode::Trilinear(lod) => Some(lod),
            SampleMode::Auto | SampleMode::Aniso(_) => Some(lod_from_derivatives(
                self.base.0,
                self.base.1,
                d.du_x,
                d.dv_x,
                d.du_y,
                d.dv_y,
            )),
            SampleMode::Nearest | SampleMode::Bilinear => None,
        };
        let texel = match self.mode {
            SampleMode::Nearest => self.mip.sample_nearest(u, v),
            SampleMode::Bilinear => self.mip.sample_trilinear(u, v, 0.0),
            SampleMode::Trilinear(_) | SampleMode::Auto => {
                self.mip.sample_trilinear(u, v, lod.unwrap_or(0.0))
            }
            SampleMode::Aniso(taps) => self
                .mip
                .sample_aniso(u, v, d.du_x, d.dv_x, d.du_y, d.dv_y, taps),
        }
        .expect("mip chain is non-empty (checked by render_textured)");
        Shaded {
            color: [texel[0], texel[1], texel[2]],
            lod,
        }
    }
}

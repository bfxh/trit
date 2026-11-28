//! 输出端色彩管理：传递函数、曝光、白平衡、饱和度、色域转换与 tone mapping。
//!
//! 管线：输入解码（可选）→ 曝光 → 白平衡 → 饱和度 → 色域转换 → tone map →
//! 输出编码（可选）。源原色按 sRGB/Rec.709 解释；默认 `raw`/`raw`、sRGB 色域、
//! EV=0、无白平衡、饱和度=1、`None` 为恒等，不改变既有输出。

mod gamut;
mod icc;
mod tone;
mod transfer;
mod white_balance;

pub use gamut::Gamut;
pub use icc::profile as icc_profile;
pub use transfer::{bt1886_decode, bt1886_encode, srgb_decode, srgb_encode};
pub use white_balance::{WhiteBalance, white_balance};

use crate::Frame;

/// 输入或输出的传递函数。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorSpace {
    /// 不做传递函数变换（沿用 v9 的直接 8bit 口径）。
    Raw,
    /// sRGB（IEC 61966-2-1）传递函数。
    Srgb,
    /// BT.1886 纯 gamma 2.4（Lb=0 简化），用于 8-bit Rec.2020 SDR。
    Bt1886,
}

/// 线性光 tone mapping。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToneMap {
    /// 不压缩，仅由输出端截断。
    None,
    /// Reinhard：`x / (1 + x)`。
    Reinhard,
    /// ACES filmic（Narkowicz 近似）。
    Aces,
}

/// 色彩处理参数；默认恒等。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColorConfig {
    pub input: ColorSpace,
    pub output: ColorSpace,
    /// 输出原色色域；默认 sRGB/Rec.709，不做转换。
    pub output_gamut: Gamut,
    /// 曝光补偿，单位 EV（2^EV 线性增益）。
    pub exposure_ev: f32,
    /// 白平衡；默认中性。
    pub white_balance: WhiteBalance,
    /// 饱和度，1 为原样，0 为灰度。
    pub saturation: f32,
    pub tone_map: ToneMap,
}

impl Default for ColorConfig {
    fn default() -> Self {
        Self {
            input: ColorSpace::Raw,
            output: ColorSpace::Raw,
            output_gamut: Gamut::Srgb,
            exposure_ev: 0.0,
            white_balance: WhiteBalance::default(),
            saturation: 1.0,
            tone_map: ToneMap::None,
        }
    }
}

impl ColorConfig {
    /// 是否完全不改变像素（包括输入/输出传递函数与色域）。
    pub fn is_identity(&self) -> bool {
        self.input == ColorSpace::Raw
            && self.output == ColorSpace::Raw
            && self.output_gamut == Gamut::Srgb
            && self.exposure_ev == 0.0
            && self.white_balance.temperature_k <= 0.0
            && self.white_balance.tint == 0.0
            && self.saturation == 1.0
            && self.tone_map == ToneMap::None
    }

    /// 原地处理整帧 RGB8。
    pub fn apply(&self, frame: &mut Frame) {
        if self.is_identity() {
            return;
        }
        let gain = 2.0f32.powf(self.exposure_ev);
        let wb = if self.white_balance.temperature_k > 0.0 {
            white_balance(self.white_balance.temperature_k, self.white_balance.tint)
        } else if self.white_balance.tint != 0.0 {
            white_balance(6500.0, self.white_balance.tint)
        } else {
            [1.0; 3]
        };
        for px in frame.rgb.as_chunks_mut::<3>().0 {
            let mut c = [
                px[0] as f32 / 255.0,
                px[1] as f32 / 255.0,
                px[2] as f32 / 255.0,
            ];
            c = match self.input {
                ColorSpace::Raw => c,
                ColorSpace::Srgb => c.map(srgb_decode),
                ColorSpace::Bt1886 => c.map(bt1886_decode),
            };
            c = [
                c[0] * gain * wb[0],
                c[1] * gain * wb[1],
                c[2] * gain * wb[2],
            ];
            let luma = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
            c = c.map(|v| luma + (v - luma) * self.saturation);
            c = gamut::convert(c, self.output_gamut);
            c = c.map(|v| tone::map_channel(v, self.tone_map));
            c = match self.output {
                ColorSpace::Raw => c,
                ColorSpace::Srgb => c.map(srgb_encode),
                ColorSpace::Bt1886 => c.map(bt1886_encode),
            };
            for (dst, v) in px.iter_mut().zip(c) {
                *dst = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            }
        }
    }
}

#[cfg(test)]
#[path = "color/tests/mod.rs"]
mod tests;

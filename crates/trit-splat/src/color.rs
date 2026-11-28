//! 输出端色彩管理：输入/输出传递函数、曝光、白平衡、饱和度与 tone mapping。
//!
//! 管线：输入解码（可选）→ 曝光 → 白平衡 → 饱和度 → tone map → 输出编码（可选）。
//! 默认 `raw`/`raw`、EV=0、无白平衡、饱和度=1、`None` 为恒等，不改变既有输出。

use crate::Frame;

/// 输入或输出的传递函数。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorSpace {
    /// 不做传递函数变换（沿用 v9 的直接 8bit 口径）。
    Raw,
    /// sRGB（IEC 61966-2-1）传递函数。
    Srgb,
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
    /// 曝光补偿，单位 EV（2^EV 线性增益）。
    pub exposure_ev: f32,
    /// 白平衡色温，单位 K；0 表示关闭。
    pub temperature_k: f32,
    /// 绿-洋红色调，[-1, 1]；0 表示中性。
    pub tint: f32,
    /// 饱和度，1 为原样，0 为灰度。
    pub saturation: f32,
    pub tone_map: ToneMap,
}

impl Default for ColorConfig {
    fn default() -> Self {
        Self {
            input: ColorSpace::Raw,
            output: ColorSpace::Raw,
            exposure_ev: 0.0,
            temperature_k: 0.0,
            tint: 0.0,
            saturation: 1.0,
            tone_map: ToneMap::None,
        }
    }
}

impl ColorConfig {
    /// 是否完全不改变像素（包括输入/输出传递函数）。
    pub fn is_identity(&self) -> bool {
        self.input == ColorSpace::Raw
            && self.output == ColorSpace::Raw
            && self.exposure_ev == 0.0
            && self.temperature_k <= 0.0
            && self.tint == 0.0
            && self.saturation == 1.0
            && self.tone_map == ToneMap::None
    }

    /// 原地处理整帧 RGB8。
    pub fn apply(&self, frame: &mut Frame) {
        if self.is_identity() {
            return;
        }
        let gain = 2.0f32.powf(self.exposure_ev);
        let wb = if self.temperature_k > 0.0 {
            white_balance(self.temperature_k, self.tint)
        } else if self.tint != 0.0 {
            white_balance(6500.0, self.tint)
        } else {
            [1.0; 3]
        };
        for px in frame.rgb.as_chunks_mut::<3>().0 {
            let mut c = [
                px[0] as f32 / 255.0,
                px[1] as f32 / 255.0,
                px[2] as f32 / 255.0,
            ];
            if self.input == ColorSpace::Srgb {
                c = c.map(srgb_decode);
            }
            c = [
                c[0] * gain * wb[0],
                c[1] * gain * wb[1],
                c[2] * gain * wb[2],
            ];
            let luma = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
            c = c.map(|v| luma + (v - luma) * self.saturation);
            c = c.map(|v| tone_map_channel(v, self.tone_map));
            if self.output == ColorSpace::Srgb {
                c = c.map(srgb_encode);
            }
            for (dst, v) in px.iter_mut().zip(c) {
                *dst = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            }
        }
    }
}

/// sRGB 编码：线性光 → sRGB。
pub fn srgb_encode(v: f32) -> f32 {
    if v <= 0.003_130_8 {
        12.92 * v
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

/// sRGB 解码：sRGB → 线性光。
pub fn srgb_decode(v: f32) -> f32 {
    if v <= 0.040_45 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn tone_map_channel(v: f32, mode: ToneMap) -> f32 {
    let v = v.max(0.0);
    match mode {
        ToneMap::None => v,
        ToneMap::Reinhard => v / (1.0 + v),
        ToneMap::Aces => (v * (2.51 * v + 0.03)) / (v * (2.43 * v + 0.59) + 0.14),
    }
}

/// 色温/色调 → 线性 RGB 增益；以 6500K 为中性。
pub fn white_balance(temperature_k: f32, tint: f32) -> [f32; 3] {
    let target = kelvin_rgb(temperature_k.max(1.0));
    let neutral = kelvin_rgb(6500.0);
    let mut wb = [
        target[0] / neutral[0],
        target[1] / neutral[1],
        target[2] / neutral[2],
    ];
    let tint = tint.clamp(-1.0, 1.0);
    wb[1] *= 1.0 + 0.5 * tint;
    wb[0] *= 1.0 - 0.25 * tint;
    wb[2] *= 1.0 - 0.25 * tint;
    wb
}

/// Tanner Helland 黑体色温近似，输出 0..1 RGB。
fn kelvin_rgb(k: f32) -> [f32; 3] {
    let t = (k / 100.0).clamp(10.0, 400.0);
    let r = if t <= 66.0 {
        255.0
    } else {
        329.698_73 * (t - 60.0).powf(-0.133_204_76)
    };
    let g = if t <= 66.0 {
        99.470_8 * t.ln() - 161.119_57
    } else {
        288.122_17 * (t - 60.0).powf(-0.075_514_85)
    };
    let b = if t >= 66.0 {
        255.0
    } else if t <= 19.0 {
        0.0
    } else {
        138.517_73 * (t - 10.0).ln() - 305.044_8
    };
    [r, g, b].map(|v| (v / 255.0).clamp(0.0, 1.0))
}

#[cfg(test)]
#[path = "color/tests.rs"]
mod tests;

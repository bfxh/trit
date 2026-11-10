/// 五个**显式**风格旋钮（S6.5）：不做生成，只做可测、可逆的图像变换。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Style {
    /// 对比度倍数（围绕均值缩放；1.0 = 不变）
    pub contrast: f32,
    /// 色阶数（量化为 N 档；0 或 1 = 不变）
    pub levels: u32,
    /// 色温（R/B 反向增益；0 = 不变，正 = 偏暖）
    pub warmth: f32,
    /// 饱和度（0 = 灰度，1 = 不变）
    pub sat: f32,
    /// 笔触尺度（模糊半径，像素；0 = 不变，负值 = 锐化）
    pub stroke: f32,
}

impl Default for Style {
    fn default() -> Self {
        Style {
            contrast: 1.0,
            levels: 0,
            warmth: 0.0,
            sat: 1.0,
            stroke: 0.0,
        }
    }
}

impl Style {
    pub fn is_identity(&self) -> bool {
        *self == Style::default()
    }
}

fn box_blur(px: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    if r == 0 {
        return px.to_vec();
    }
    let mut tmp = vec![0.0f32; px.len()];
    let n = (2 * r + 1) as f32;
    for y in 0..h {
        for x in 0..w {
            for c in 0..3 {
                let mut s = 0.0f32;
                for k in 0..=2 * r {
                    let xx = (x + k).saturating_sub(r).min(w - 1);
                    s += px[(y * w + xx) * 3 + c];
                }
                tmp[(y * w + x) * 3 + c] = s / n;
            }
        }
    }
    let mut out = vec![0.0f32; px.len()];
    for y in 0..h {
        for x in 0..w {
            for c in 0..3 {
                let mut s = 0.0f32;
                for k in 0..=2 * r {
                    let yy = (y + k).saturating_sub(r).min(h - 1);
                    s += tmp[(yy * w + x) * 3 + c];
                }
                out[(y * w + x) * 3 + c] = s / n;
            }
        }
    }
    out
}

/// 按 [`Style`] 做**显式**变换（顺序固定：笔触 → 对比度 → 色温 → 饱和 → **色阶**）。
/// 色阶必须放最后（posterize 语义）：放前面的话后面的线性变换会再造出新值，量化档数测不出来。
///
/// 每一步都是可逆/单调的闭式变换，不引入随机性；参数为默认值时逐位不变。
pub fn stylize(px: &[f32], w: usize, h: usize, s: &Style) -> Vec<f32> {
    let mut out = px.to_vec();
    // 分数半径：在 r 与 r+1 之间线性混合（直接 round() 会让 0.5 突然变整级模糊，实测对比度断崖）
    let frac_blur = |src: &[f32], r: f32| -> Vec<f32> {
        let r = r.max(0.0);
        let lo = r.floor() as usize;
        let fr = r - lo as f32;
        let a = box_blur(src, w, h, lo);
        if fr <= 1e-6 {
            return a;
        }
        let b = box_blur(src, w, h, lo + 1);
        a.iter()
            .zip(&b)
            .map(|(x, y)| x * (1.0 - fr) + y * fr)
            .collect()
    };
    if s.stroke > 0.0 {
        out = frac_blur(&out, s.stroke);
    } else if s.stroke < 0.0 {
        // 负值 = 锐化：原图 + (原图 − 模糊)
        let b = frac_blur(px, -s.stroke);
        for i in 0..out.len() {
            out[i] = (px[i] * 2.0 - b[i]).clamp(0.0, 1.0);
        }
    }
    if (s.contrast - 1.0).abs() > 1e-6 {
        let n = (w * h) as f32;
        let mut mean = [0.0f32; 3];
        for p in out.chunks(3) {
            for c in 0..3 {
                mean[c] += p[c];
            }
        }
        for m in mean.iter_mut() {
            *m /= n;
        }
        for p in out.chunks_mut(3) {
            for c in 0..3 {
                p[c] = ((p[c] - mean[c]) * s.contrast + mean[c]).clamp(0.0, 1.0);
            }
        }
    }
    if s.warmth.abs() > 1e-6 {
        for p in out.chunks_mut(3) {
            p[0] = (p[0] * (1.0 + s.warmth)).clamp(0.0, 1.0);
            p[2] = (p[2] * (1.0 - s.warmth)).clamp(0.0, 1.0);
        }
    }
    if (s.sat - 1.0).abs() > 1e-6 {
        for p in out.chunks_mut(3) {
            let y = 0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2];
            for v in p.iter_mut() {
                *v = (y + (*v - y) * s.sat).clamp(0.0, 1.0);
            }
        }
    }
    if s.levels >= 2 {
        // 在**图自身**的 [lo, hi] 里量化：按 0..1 量化会把深色图压成 1–2 档（实测 8 档只剩 2 档）
        let lo = out.iter().cloned().fold(f32::INFINITY, f32::min);
        let hi = out.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        if hi - lo > 1e-6 {
            let n = s.levels as f32 - 1.0;
            for v in out.iter_mut() {
                let t = ((*v - lo) / (hi - lo) * n).round() / n;
                *v = lo + t * (hi - lo);
            }
        }
    }
    out
}

/// **点式**风格（无空间操作）：对比度 → 色温 → 饱和 → 色阶（最后 posterize）。
/// 用于「按材质类别分开上风格」——类别是逐三角形的，模糊这类空间算子不能逐三角形做。
pub fn apply_pointwise(px: &mut [f32], s: &Style) {
    if (s.contrast - 1.0).abs() > 1e-6 {
        let n = (px.len() / 3) as f32;
        let mut mean = [0.0f32; 3];
        for p in px.chunks(3) {
            for c in 0..3 {
                mean[c] += p[c];
            }
        }
        for m in mean.iter_mut() {
            *m /= n;
        }
        for p in px.chunks_mut(3) {
            for c in 0..3 {
                p[c] = ((p[c] - mean[c]) * s.contrast + mean[c]).clamp(0.0, 1.0);
            }
        }
    }
    if s.warmth.abs() > 1e-6 {
        for p in px.chunks_mut(3) {
            p[0] = (p[0] * (1.0 + s.warmth)).clamp(0.0, 1.0);
            p[2] = (p[2] * (1.0 - s.warmth)).clamp(0.0, 1.0);
        }
    }
    if (s.sat - 1.0).abs() > 1e-6 {
        for p in px.chunks_mut(3) {
            let y = 0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2];
            for v in p.iter_mut() {
                *v = (y + (*v - y) * s.sat).clamp(0.0, 1.0);
            }
        }
    }
    if s.levels >= 2 {
        let lo = px.iter().cloned().fold(f32::INFINITY, f32::min);
        let hi = px.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        if hi - lo > 1e-6 {
            let n = s.levels as f32 - 1.0;
            for v in px.iter_mut() {
                let t = ((*v - lo) / (hi - lo) * n).round() / n;
                *v = lo + t * (hi - lo);
            }
        }
    }
}

/// 风格预设（S6.7 止损后的形态）：**固定变换序列 + 单一强度 `t ∈ [0,1]`**。
///
/// 三个预设只在「端点」定义（t=1），中间按 t 线性插值；t=0 必须逐位等于原图（单测钉住）。
#[derive(Clone, Copy, Debug)]
pub struct Preset {
    pub name: &'static str,
    /// t=1 时的对比度倍数
    pub contrast: f32,
    /// t=1 时的色阶数（0 = 不量化）
    pub levels: u32,
    /// t=1 时的色温
    pub warmth: f32,
    /// t=1 时的饱和度
    pub sat: f32,
    /// t=1 时的笔触（正 = 模糊、负 = 锐化，0 = 不变）
    pub stroke: f32,
}

/// 三个预设：赛璐璐 / 油画 / 史诗写实。
pub const PRESETS: [Preset; 3] = [
    Preset {
        name: "cel",
        contrast: 1.5,
        levels: 6,
        warmth: 0.0,
        sat: 1.25,
        stroke: -1.0,
    },
    Preset {
        name: "oil",
        contrast: 1.15,
        levels: 0,
        warmth: 0.12,
        sat: 1.2,
        stroke: 3.0,
    },
    Preset {
        name: "epic",
        // 实测两条：① 量化放最后会压掉局部对比度 ⇒ 不做 posterize；
        // ② 任何模糊都会抵消对比度增益（t=1 时 contrast 0.29→0.24）⇒ 这个预设不带笔触。
        contrast: 1.35,
        levels: 0,
        warmth: 0.18,
        sat: 1.05,
        stroke: 0.0,
    },
];

impl Preset {
    pub fn by_name(name: &str) -> Option<Preset> {
        PRESETS.iter().copied().find(|p| p.name == name)
    }

    /// 强度 `t` 下的显式参数（t=0 ⇒ 默认参数；`levels` 从 32 档线性收拢到端点档数）。
    pub fn style(&self, t: f32) -> Style {
        let t = t.clamp(0.0, 1.0);
        let levels = if self.levels < 2 || t <= 0.0 {
            0
        } else {
            let hi = 32.0f32;
            let lo = self.levels as f32;
            (hi + (lo - hi) * t).round().max(2.0) as u32
        };
        Style {
            contrast: 1.0 + (self.contrast - 1.0) * t,
            levels,
            warmth: self.warmth * t,
            sat: 1.0 + (self.sat - 1.0) * t,
            stroke: self.stroke * t,
        }
    }
}

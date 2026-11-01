//! 程序化材质（N5 的第一块）：可平铺的 2D 纹理 + 无缝度判据。
//!
//! 全部用**周期函数**构造（周期 = 贴图边长），所以左右/上下边界天然连续；
//! `seam_ratio` 是判据：边界相邻差 / 内部相邻差，≈1 表示无缝。
//! `quilt` 是**非参数**合成：直接从源图搬 patch，统计由构造保证（学出来的合成见 `tconv`）。

/// splitmix64：给 patch 搜索用（`seed | 1` 会丢最低位，2026-11-01 踩过）。
fn sm64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// 非参数 patch 合成（Efros–Freeman 随机候选 + 重叠 SSD 选优 + 线性羽化）。
///
/// `src` 为交错 RGB（0..1，`sw×sh`）；输出 `out_w×out_h` 同格式。
/// 每个 patch 只随机试 `tries` 个候选，选与已落区域 SSD 最小的那个 ⇒ 确定性（同 seed 逐位一致）。
#[allow(clippy::too_many_arguments)]
pub fn quilt(
    src: &[f32],
    sw: usize,
    sh: usize,
    out_w: usize,
    out_h: usize,
    patch: usize,
    overlap: usize,
    seed: u64,
    tone_match: bool,
    detrend: usize,
    relight: bool,
) -> Vec<f32> {
    assert!(patch >= 2 && patch <= sw.min(sh), "patch 必须 ≤ 源图短边");
    assert!(out_w >= patch && out_h >= patch, "输出不能小于一个 patch");
    let ov = overlap.min(patch - 1);
    let step = patch - ov;
    let mut out = vec![0.0f32; out_w * out_h * 3];
    let mut placed = vec![false; out_w * out_h];
    let mut xs = Vec::new();
    let mut x = 0;
    while x + patch < out_w {
        xs.push(x);
        x += step;
    }
    xs.push(out_w - patch);
    let mut ys = Vec::new();
    let mut y = 0;
    while y + patch < out_h {
        ys.push(y);
        y += step;
    }
    ys.push(out_h - patch);
    // 去低频（光照梯度）：拟合并扣除线性光照，拼完再回贴 —— 拼接口会抹平跨区域光照，
    // 这是对比度 p95−p5 大幅缩水的主因（Planks008 实测 −78%）。

    let src_orig: &[f32] = src;
    let work: Vec<f32> = if detrend > 0 {
        let lf = box_blur_rgb(src, sw, sh, detrend);
        src.iter().zip(&lf).map(|(a, b)| a - b).collect()
    } else {
        src.to_vec()
    };
    let src: &[f32] = &work;
    let mut state = seed;
    let tries = 64u64;
    // 源图逐通道均值/方差：tone-match 时把每个 patch 拉到同一均值/方差，
    // 抵消「不同区域的光照梯度在拼接时被抹平」（实测 contrast p95−p5 可掉 65–78%）
    let gmean = if tone_match {
        let n = (sw * sh) as f32;
        let mut m = [0.0f32; 3];
        for p in src.chunks(3) {
            for c in 0..3 {
                m[c] += p[c];
            }
        }
        for v in m.iter_mut() {
            *v /= n;
        }
        Some(m)
    } else {
        None
    };
    let mut src_std = [0.0f32; 3];
    if let Some(gm) = gmean {
        let n = (sw * sh) as f32;
        let mut v = [0.0f32; 3];
        for p in src.chunks(3) {
            for c in 0..3 {
                v[c] += (p[c] - gm[c]).powi(2);
            }
        }
        for c in 0..3 {
            src_std[c] = (v[c] / n).sqrt().max(1e-4);
        }
    }
    for &oy in &ys {
        for &ox in &xs {
            let mut best = (f32::INFINITY, 0usize, 0usize);
            for _ in 0..tries {
                let cx = (sm64(&mut state) % (sw - patch + 1) as u64) as usize;
                let cy = (sm64(&mut state) % (sh - patch + 1) as u64) as usize;
                let mut cost = 0.0f32;
                for dy in 0..patch {
                    let gy = oy + dy;
                    for dx in 0..patch {
                        let gx = ox + dx;
                        if !placed[gy * out_w + gx] {
                            continue;
                        }
                        let s = ((cy + dy) * sw + cx + dx) * 3;
                        let o = (gy * out_w + gx) * 3;
                        for c in 0..3 {
                            let d = src[s + c] - out[o + c];
                            cost += d * d;
                        }
                    }
                }
                if cost < best.0 {
                    best = (cost, cx, cy);
                }
            }
            let (_, cx, cy) = best;
            // 选定 patch 的逐通道均值/方差（tone-match 用）
            let pstat = if tone_match {
                let n = (patch * patch) as f32;
                let mut m = [0.0f32; 3];
                for dy in 0..patch {
                    for dx in 0..patch {
                        let s0 = ((cy + dy) * sw + cx + dx) * 3;
                        for c in 0..3 {
                            m[c] += src[s0 + c];
                        }
                    }
                }
                for v in m.iter_mut() {
                    *v /= n;
                }
                let mut vv = [0.0f32; 3];
                for dy in 0..patch {
                    for dx in 0..patch {
                        let s0 = ((cy + dy) * sw + cx + dx) * 3;
                        for c in 0..3 {
                            vv[c] += (src[s0 + c] - m[c]).powi(2);
                        }
                    }
                }
                let mut sd = [0.0f32; 3];
                for c in 0..3 {
                    sd[c] = (vv[c] / n).sqrt().max(1e-4);
                }
                Some((m, sd))
            } else {
                None
            };
            let ramp = |i: usize| (i + 1) as f32 / (ov + 1) as f32;
            for dy in 0..patch {
                let gy = oy + dy;
                for dx in 0..patch {
                    let gx = ox + dx;
                    let s = ((cy + dy) * sw + cx + dx) * 3;
                    let o = (gy * out_w + gx) * 3;
                    let mut a = 1.0f32;
                    if dy < ov {
                        a = a.min(ramp(dy));
                    }
                    if dx < ov {
                        a = a.min(ramp(dx));
                    }
                    if !placed[gy * out_w + gx] {
                        a = 1.0;
                    }
                    for c in 0..3 {
                        let mut v = src[s + c];
                        if let (Some((pm, ps)), Some(gm)) = (pstat, gmean) {
                            // 放大倍数 clamp 到 [0.5, 2]：否则低方差 patch 会把噪声放大成"结构"（实测 coherence 0.865→0.326）
                            let k = (src_std[c] / ps[c]).clamp(0.5, 2.0);
                            v = (v - pm[c]) * k + gm[c];
                        }
                        out[o + c] = out[o + c] * (1.0 - a) + v * a;
                    }
                    placed[gy * out_w + gx] = true;
                }
            }
        }
    }
    if relight {
        // **亮度分位匹配（加性）**：把输出亮度分位对到源亮度分布，位移量加到 RGB 上。
        // 逐通道独立映射也能补回对比度，但会改色相与梯度（实测 coh 0.865→0.470、hf1 +94%）⇒ 用加性保结构。
        let mut src_luma: Vec<f32> = src_orig.chunks(3).map(luma_of).collect();
        src_luma.sort_by(f32::total_cmp);
        let mut out_sorted: Vec<f32> = out.chunks(3).map(luma_of).collect();
        out_sorted.sort_by(f32::total_cmp);
        let (ns, no) = (src_luma.len(), out_sorted.len());
        if ns > 1 && no > 1 {
            for i in 0..out.len() / 3 {
                let o = i * 3;
                let y0 = luma_of(&out[o..o + 3]);
                let rank = out_sorted.partition_point(|v| *v < y0).min(no - 1);
                let q = rank as f32 / (no - 1) as f32;
                let t = (q * (ns - 1) as f32).round() as usize;
                let delta = src_luma[t.min(ns - 1)] - y0;
                for c in 0..3 {
                    out[o + c] = (out[o + c] + delta).clamp(0.0, 1.0);
                }
            }
        }
    }
    out
}

fn luma_of(p: &[f32]) -> f32 {
    0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2]
}

/// 逐通道盒式模糊（去光照用的低频）。
fn box_blur_rgb(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    if r == 0 {
        return src.to_vec();
    }
    let mut tmp = vec![0.0f32; src.len()];
    let n = (2 * r + 1) as f32;
    for y in 0..h {
        for x in 0..w {
            for c in 0..3 {
                let mut sum = 0.0f32;
                for k in 0..=2 * r {
                    let xx = (x + k).saturating_sub(r).min(w - 1);
                    sum += src[(y * w + xx) * 3 + c];
                }
                tmp[(y * w + x) * 3 + c] = sum / n;
            }
        }
    }
    let mut out = vec![0.0f32; src.len()];
    for y in 0..h {
        for x in 0..w {
            for c in 0..3 {
                let mut sum = 0.0f32;
                for k in 0..=2 * r {
                    let yy = (y + k).saturating_sub(r).min(h - 1);
                    sum += tmp[(yy * w + x) * 3 + c];
                }
                out[(y * w + x) * 3 + c] = sum / n;
            }
        }
    }
    out
}

/// 生成 `n×n` RGB 贴图（行优先，0..255）。
pub fn tile(kind: &str, n: usize, seed: u64) -> Vec<[u8; 3]> {
    let mut out = Vec::with_capacity(n * n);
    let s = (seed % 997) as f32 * 0.017;
    for y in 0..n {
        for x in 0..n {
            let u = x as f32 / n as f32;
            let v = y as f32 / n as f32;
            let (r, g, b) = match kind {
                "wood" => {
                    // 年轮：沿 x 的正弦 + 周期扰动
                    let grain = ((u * std::f32::consts::TAU * 6.0
                        + (v * std::f32::consts::TAU).sin() * 1.5
                        + s)
                        .sin()
                        * 0.5
                        + 0.5)
                        .powf(1.6);
                    (
                        0.45 + 0.35 * grain,
                        0.28 + 0.22 * grain,
                        0.12 + 0.10 * grain,
                    )
                }
                "marble" => {
                    let t = (u * std::f32::consts::TAU * 3.0
                        + (v * std::f32::consts::TAU * 2.0).sin() * 2.2
                        + s)
                        .sin();
                    let veins = (t.abs()).powf(0.35);
                    (
                        0.82 + 0.15 * veins,
                        0.82 + 0.15 * veins,
                        0.86 + 0.12 * veins,
                    )
                }
                "rust" => {
                    let a = (u * std::f32::consts::TAU * 5.0).sin()
                        * (v * std::f32::consts::TAU * 4.0).cos();
                    let b2 = ((u + v) * std::f32::consts::TAU * 3.0 + s).sin();
                    let t = (0.5 + 0.25 * a + 0.25 * b2).clamp(0.0, 1.0);
                    (0.55 + 0.35 * t, 0.22 + 0.18 * t, 0.08 + 0.08 * t)
                }
                _ => {
                    // grid：细网格线，线宽固定
                    let fx = (u * 8.0).fract();
                    let fy = (v * 8.0).fract();
                    let line = (fx < 0.06 || fy < 0.06) as u8 as f32;
                    (0.25 + 0.55 * line, 0.5 + 0.35 * line, 0.75 + 0.2 * line)
                }
            };
            out.push([
                (r.clamp(0.0, 1.0) * 255.0) as u8,
                (g.clamp(0.0, 1.0) * 255.0) as u8,
                (b.clamp(0.0, 1.0) * 255.0) as u8,
            ]);
        }
    }
    out
}

/// 无缝度：左右 + 上下边界的相邻差 / 内部相邻差。≈1 表示看不出接缝。
pub fn seam_ratio(px: &[[u8; 3]], n: usize) -> f32 {
    let diff = |a: [u8; 3], b: [u8; 3]| -> f32 {
        (0..3)
            .map(|k| (a[k] as f32 - b[k] as f32).abs())
            .sum::<f32>()
            / 3.0
    };
    let mut edge = 0.0f32;
    let mut edge_n = 0usize;
    for y in 0..n {
        edge += diff(px[y * n], px[y * n + n - 1]);
        edge_n += 1;
    }
    for x in 0..n {
        edge += diff(px[x], px[(n - 1) * n + x]);
        edge_n += 1;
    }
    // 内部相邻差取 p95（尖锐纹理的台阶本来就有大跳变，用均值会低估）
    let mut inner: Vec<f32> = Vec::new();
    for y in 0..n {
        for x in 0..n.saturating_sub(1) {
            inner.push(diff(px[y * n + x], px[y * n + x + 1]));
        }
    }
    inner.sort_by(f32::total_cmp);
    let p95 = inner
        .get((inner.len() as f32 * 0.95) as usize)
        .copied()
        .unwrap_or(0.0);
    let e = edge / edge_n.max(1) as f32;
    if p95 <= 1e-6 { f32::INFINITY } else { e / p95 }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_are_seamless_and_distinct() {
        for kind in ["wood", "marble", "rust", "grid"] {
            let t = tile(kind, 128, 7);
            assert_eq!(t.len(), 128 * 128);
            // 周期函数 ⇒ 边界差与内部差同量级（无硬接缝）
            let r = seam_ratio(&t, 128);
            assert!(r < 2.0, "{kind} 接缝比 {r:.2}");
        }
        let a = tile("wood", 64, 1);
        let b = tile("marble", 64, 1);
        let d: u32 = a
            .iter()
            .zip(&b)
            .map(|(x, y)| {
                (0..3)
                    .map(|k| (x[k] as i32 - y[k] as i32).unsigned_abs())
                    .sum::<u32>()
            })
            .sum();
        assert!(d > 64 * 64 * 30, "两种材质应有明显差异（总差 {d}）");
    }

    fn stripes(n: usize, period: usize) -> Vec<f32> {
        let mut v = Vec::with_capacity(n * n * 3);
        for _y in 0..n {
            for x in 0..n {
                let c = if (x / (period / 2)) % 2 == 0 {
                    0.85
                } else {
                    0.15
                };
                v.extend_from_slice(&[c, c, c]);
            }
        }
        v
    }

    fn sample_img(n: usize) -> Vec<f32> {
        // 有方向性 + 多色阶的合成图（亮条纹 + 噪声），便于测因子
        let mut st = 12345u64;
        let mut v = Vec::with_capacity(n * n * 3);
        for y in 0..n {
            for x in 0..n {
                let stripe = if (x / 6) % 2 == 0 { 0.75 } else { 0.35 };
                let g = sm64(&mut st) % 1000;
                let noise = (g as f32 / 1000.0 - 0.5) * 0.08;
                let base = (stripe + noise).clamp(0.0, 1.0);
                v.extend_from_slice(&[base, base * 0.9 + 0.05, base * 0.75 + 0.1]);
            }
            let _ = y;
        }
        v
    }

    #[test]
    fn preset_zero_strength_is_identity() {
        let img = sample_img(32);
        for p in PRESETS {
            assert_eq!(
                p.style(0.0),
                Style::default(),
                "{}: t=0 应为默认参数",
                p.name
            );
            assert_eq!(
                stylize(&img, 32, 32, &p.style(0.0)),
                img,
                "{}: t=0 必须逐位不变",
                p.name
            );
        }
    }

    #[test]
    fn preset_names_are_unique() {
        let mut names: Vec<&str> = PRESETS.iter().map(|p| p.name).collect();
        names.sort_unstable();
        let n = names.len();
        names.dedup();
        assert_eq!(names.len(), n, "预设名不能重复");
        assert!(Preset::by_name("cel").is_some());
        assert!(Preset::by_name("nope").is_none());
    }

    #[test]
    fn preset_strength_is_monotone_in_its_defining_factors() {
        let (w, h) = (64usize, 64usize);
        // 细纹理彩色底：stroke 不撞上限，色阶/对比度也都有空间
        let mut st = 4242u64;
        // 中调区噪声（0.25–0.75）：对比度拉到 1.35 也不会削顶，否则裁剪会让 contrast 指标假跌
        let img: Vec<f32> = (0..w * h * 3)
            .map(|i| {
                let base = 0.25 + 0.5 * ((sm64(&mut st) >> 40) as f32 / (1u64 << 24) as f32);
                base * [1.0, 0.9, 0.8][i % 3]
            })
            .collect();
        let ts = [0.0f32, 0.25, 0.5, 0.75, 1.0];
        for p in PRESETS {
            let v: Vec<crate::look::Look> = ts
                .iter()
                .map(|&t| crate::look::analyze(&stylize(&img, w, h, &p.style(t)), w, h))
                .collect();
            match p.name {
                "cel" => {
                    // look.levels 是 luma 直方图档数，逐通道量化后 luma 会组合出更多值；
                    // 验收「posterize」要看**通道实际离散值数** ≤ 请求档数，并随强度单调减少。
                    let distinct: Vec<usize> = ts
                        .iter()
                        .map(|&t| {
                            let o = stylize(&img, w, h, &p.style(t));
                            let mut set: Vec<u32> = o.iter().map(|v| (v * 1e6) as u32).collect();
                            set.sort_unstable();
                            set.dedup();
                            set.len()
                        })
                        .collect();
                    let mono = distinct.windows(2).all(|x| x[1] <= x[0]);
                    assert!(mono, "cel 量化档数应随强度单调减少，实际 {distinct:?}");
                    for (i, &d) in distinct.iter().enumerate() {
                        let req = p.style(ts[i]).levels as usize;
                        let want = if req == 0 { usize::MAX } else { req };
                        assert!(d <= want, "t={}: 离散值 {d} 应 ≤ 请求档数 {want}", ts[i]);
                    }
                    let contrast_up = v.windows(2).all(|x| x[1].contrast >= x[0].contrast - 1e-3);
                    assert!(
                        contrast_up,
                        "cel 对比度应单调上升，实际 {:?}",
                        v.iter().map(|l| l.contrast).collect::<Vec<_>>()
                    );
                }
                "oil" => {
                    let s_up = v.windows(2).all(|x| x[1].stroke >= x[0].stroke - 1e-3);
                    assert!(
                        s_up,
                        "oil 笔触尺度应单调上升，实际 {:?}",
                        v.iter().map(|l| l.stroke).collect::<Vec<_>>()
                    );
                    let w_up = v.windows(2).all(|x| x[1].warmth >= x[0].warmth - 1e-3);
                    assert!(
                        w_up,
                        "oil 色温应单调上升，实际 {:?}",
                        v.iter().map(|l| l.warmth).collect::<Vec<_>>()
                    );
                }
                "epic" => {
                    let c_up = v.windows(2).all(|x| x[1].contrast >= x[0].contrast - 1e-3);
                    assert!(
                        c_up,
                        "epic 对比度应单调上升，实际 {:?}",
                        v.iter().map(|l| l.contrast).collect::<Vec<_>>()
                    );
                    let w_up = v.windows(2).all(|x| x[1].warmth >= x[0].warmth - 1e-3);
                    assert!(
                        w_up,
                        "epic 色温应单调上升，实际 {:?}",
                        v.iter().map(|l| l.warmth).collect::<Vec<_>>()
                    );
                }
                _ => unreachable!(),
            }
        }
    }

    #[test]
    fn stylize_identity_is_bit_exact() {
        let img = sample_img(32);
        let out = stylize(&img, 32, 32, &Style::default());
        assert_eq!(out, img, "默认参数必须逐位不变");
    }

    #[test]
    fn stylize_knobs_move_their_own_factor_monotonically() {
        let (n, w, h) = (48usize, 48usize, 48usize);
        let img = sample_img(n);
        let look = |s: &Style| {
            let o = stylize(&img, w, h, s);
            crate::look::analyze(&o, w, h)
        };
        let base = look(&Style::default());

        let lo = look(&Style {
            levels: 4,
            ..Default::default()
        });
        let hi = look(&Style {
            levels: 32,
            ..Default::default()
        });
        assert!(
            hi.levels > lo.levels,
            "色阶：32 档应比 4 档测出更多级数（{} vs {}）",
            hi.levels,
            lo.levels
        );

        let c1 = look(&Style {
            contrast: 0.6,
            ..Default::default()
        });
        let c2 = look(&Style {
            contrast: 1.8,
            ..Default::default()
        });
        assert!(
            c2.contrast > c1.contrast,
            "对比度应单调（{} vs {}）",
            c2.contrast,
            c1.contrast
        );

        let warm = look(&Style {
            warmth: 0.25,
            ..Default::default()
        });
        assert!(
            warm.warmth > base.warmth,
            "色温应上升（{} vs {}）",
            warm.warmth,
            base.warmth
        );

        let sat = look(&Style {
            sat: 1.6,
            ..Default::default()
        });
        assert!(
            sat.sat > base.sat,
            "饱和度应上升（{} vs {}）",
            sat.sat,
            base.sat
        );

        // 笔触尺度要看「相关长度」：条纹图的相关长度被条纹周期主导，改用纯噪声图
        let mut st = 999u64;
        let noise: Vec<f32> = (0..n * n * 3)
            .map(|_| (sm64(&mut st) >> 40) as f32 / (1u64 << 24) as f32)
            .collect();
        let s0 = crate::look::analyze(&noise, w, h).stroke;
        let s2 = crate::look::analyze(
            &stylize(
                &noise,
                w,
                h,
                &Style {
                    stroke: 3.0,
                    ..Default::default()
                },
            ),
            w,
            h,
        )
        .stroke;
        let s_sharp = crate::look::analyze(
            &stylize(
                &noise,
                w,
                h,
                &Style {
                    stroke: -1.0,
                    ..Default::default()
                },
            ),
            w,
            h,
        )
        .stroke;
        assert!(s2 > s0, "模糊应拉长相关长度（{} vs {}）", s2, s0);
        assert!(
            s_sharp <= s0 + 1e-3,
            "锐化不该拉长相关长度（{} vs {}）",
            s_sharp,
            s0
        );
    }

    #[test]
    fn stylize_is_deterministic() {
        let img = sample_img(24);
        let s = Style {
            contrast: 1.3,
            levels: 12,
            warmth: 0.1,
            sat: 1.2,
            stroke: 1.0,
        };
        assert_eq!(stylize(&img, 24, 24, &s), stylize(&img, 24, 24, &s));
    }

    fn ramp_with_noise(n: usize) -> Vec<f32> {
        let mut st = 77u64;
        let mut v = Vec::with_capacity(n * n * 3);
        for _y in 0..n {
            for x in 0..n {
                let ramp = 0.15 + 0.7 * (x as f32 / n as f32);
                let noise = ((sm64(&mut st) >> 40) as f32 / (1u64 << 24) as f32 - 0.5) * 0.06;
                let c = (ramp + noise).clamp(0.0, 1.0);
                v.extend_from_slice(&[c, c, c]);
            }
        }
        v
    }

    fn lr_diff(img: &[f32], n: usize) -> f32 {
        let q = n / 4;
        let mut left = 0.0f32;
        let mut right = 0.0f32;
        let mut cnt = 0.0f32;
        for y in 0..n {
            for x in 0..q {
                left += img[(y * n + x) * 3];
                right += img[(y * n + (n - 1 - x)) * 3];
                cnt += 1.0;
            }
        }
        (right - left) / cnt
    }

    #[test]
    fn detrend_removes_and_relight_restores_the_global_gradient() {
        let n = 96usize;
        let src = ramp_with_noise(n);
        let d_src = lr_diff(&src, n);
        assert!(d_src > 0.3, "测试源本身要有明显梯度，实际 {d_src}");
        // 只去梯度不回贴：输出应基本平坦
        let flat = quilt(&src, n, n, n, n, 16, 4, 5, false, 16, false);
        let d_flat = lr_diff(&flat, n).abs();
        assert!(
            d_flat < d_src * 0.35,
            "detrend 后梯度应大幅减小：{d_flat:.3} vs 源 {d_src:.3}"
        );
        // 去梯度 + 分位回贴：对比度（p95−p5）应回到源的 80% 以上
        let lit = quilt(&src, n, n, n, n, 16, 4, 5, false, 16, true);
        let p = |img: &[f32]| -> f32 {
            let mut v: Vec<f32> = img.chunks(3).map(luma_of).collect();
            v.sort_by(f32::total_cmp);
            let q = |f: f32| v[((v.len() as f32 - 1.0) * f).round() as usize];
            q(0.95) - q(0.05)
        };
        let (cs, cl) = (p(&src), p(&lit));
        assert!(
            cl >= cs * 0.8,
            "分位回贴后对比度应接近源：{cl:.3} vs {cs:.3}"
        );
    }

    #[test]
    fn quilt_outputs_the_requested_size_and_stays_in_range() {
        let src = stripes(32, 8);
        let out = quilt(&src, 32, 32, 64, 48, 8, 4, 7, false, 0, false);
        assert_eq!(out.len(), 64 * 48 * 3);
        assert!(out.iter().all(|v| (0.0..=1.0).contains(v)));
        // 源只有两档灰度 ⇒ 输出也只应落在两档附近（羽化区允许中间值）
        let mid = out.iter().filter(|v| (0.3..0.7).contains(*v)).count();
        assert!(
            mid < out.len() / 4,
            "羽化区外的值应保持源图两档，中间值 {mid}"
        );
    }

    #[test]
    fn quilt_is_deterministic_and_seed_sensitive() {
        let src = stripes(32, 8);
        let a = quilt(&src, 32, 32, 64, 64, 8, 4, 1, false, 0, false);
        let b = quilt(&src, 32, 32, 64, 64, 8, 4, 1, false, 0, false);
        assert_eq!(a, b, "同 seed 必须逐位一致");
        let c = quilt(&src, 32, 32, 64, 64, 8, 4, 2, false, 0, false);
        assert_ne!(a, c, "不同 seed 应给出不同拼法");
    }

    #[test]
    fn quilt_preserves_directionality_where_diffusion_does_not() {
        // 竖条纹源：合成图必须仍是大幅各向异性；随机噪声源作对照必须是各向同性
        let src = stripes(48, 8);
        let out = quilt(&src, 48, 48, 64, 64, 12, 6, 3, false, 0, false);
        let l = crate::look::analyze(&out, 64, 64);
        assert!(
            l.coherence > 0.6,
            "竖条纹应保住方向性，实际 {:.3}",
            l.coherence
        );
        assert!(
            l.stroke > 2.0,
            "笔触尺度应接近源的条纹宽度，实际 {:.2}",
            l.stroke
        );

        let mut st = 99u64;
        let noise: Vec<f32> = (0..48 * 48 * 3)
            .map(|_| (sm64(&mut st) >> 40) as f32 / (1u64 << 24) as f32)
            .collect();
        let l2 = crate::look::analyze(&noise, 48, 48);
        assert!(
            l2.coherence < 0.4,
            "对照：噪声应各向同性，实际 {:.3}",
            l2.coherence
        );
    }
}

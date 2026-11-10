pub(crate) fn sm64(state: &mut u64) -> u64 {
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
    relight_plane: bool,
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
    // 平面回贴（旧行为）：只在 `--relight-plane` 时用；分布回贴不需要平面。
    let plane = if relight_plane {
        fit_plane(src, sw, sh)
    } else {
        [[0.0f32; 3]; 3]
    };
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
    if relight_plane {
        // **旧行为**：回贴源图拟合的线性平面。结构保得住，但对比度只回一小部分（§6.80/§6.81）。
        for y in 0..out_h {
            let v = y as f32 / out_h.max(1) as f32;
            for x in 0..out_w {
                let u = x as f32 / out_w.max(1) as f32;
                let o = (y * out_w + x) * 3;
                for c in 0..3 {
                    let g = plane[c][0] + plane[c][1] * u + plane[c][2] * v;
                    out[o + c] = (out[o + c] + g).clamp(0.0, 1.0);
                }
            }
        }
    } else if relight {
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

pub(crate) fn luma_of(p: &[f32]) -> f32 {
    0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2]
}

/// 逐通道线性平面拟合 `a + bx·u + by·v`（u,v ∈ `[0,1]`），最小二乘闭式解（`--relight-plane` 用）。
fn fit_plane(src: &[f32], w: usize, h: usize) -> [[f32; 3]; 3] {
    let n = (w * h) as f64;
    let (mut su, mut sv, mut suu, mut svv, mut suv) = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
    let mut sy = [0.0f64; 3];
    let mut suy = [0.0f64; 3];
    let mut svy = [0.0f64; 3];
    for y in 0..h {
        let v = y as f64 / h.max(1) as f64;
        for x in 0..w {
            let u = x as f64 / w.max(1) as f64;
            su += u;
            sv += v;
            suu += u * u;
            svv += v * v;
            suv += u * v;
            let i = (y * w + x) * 3;
            for c in 0..3 {
                let val = src[i + c] as f64;
                sy[c] += val;
                suy[c] += val * u;
                svy[c] += val * v;
            }
        }
    }
    let inv = inv3([[n, su, sv], [su, suu, suv], [sv, suv, svv]]);
    let mut out = [[0.0f32; 3]; 3];
    for c in 0..3 {
        let b = [sy[c], suy[c], svy[c]];
        for row in 0..3 {
            out[c][row] = (inv[row][0] * b[0] + inv[row][1] * b[1] + inv[row][2] * b[2]) as f32;
        }
    }
    out
}

fn inv3(m: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    let d = if det.abs() < 1e-12 { 1e-12 } else { det };
    [
        [
            (m[1][1] * m[2][2] - m[1][2] * m[2][1]) / d,
            (m[0][2] * m[2][1] - m[0][1] * m[2][2]) / d,
            (m[0][1] * m[1][2] - m[0][2] * m[1][1]) / d,
        ],
        [
            (m[1][2] * m[2][0] - m[1][0] * m[2][2]) / d,
            (m[0][0] * m[2][2] - m[0][2] * m[2][0]) / d,
            (m[0][2] * m[1][0] - m[0][0] * m[1][2]) / d,
        ],
        [
            (m[1][0] * m[2][1] - m[1][1] * m[2][0]) / d,
            (m[0][1] * m[2][0] - m[0][0] * m[2][1]) / d,
            (m[0][0] * m[1][1] - m[0][1] * m[1][0]) / d,
        ],
    ]
}

/// 逐通道盒式模糊（去光照用的低频）。
pub(crate) fn box_blur_rgb(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
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

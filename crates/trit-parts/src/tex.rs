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
    let mut state = seed;
    let tries = 64u64;
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
                        out[o + c] = out[o + c] * (1.0 - a) + src[s + c] * a;
                    }
                    placed[gy * out_w + gx] = true;
                }
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

    #[test]
    fn quilt_outputs_the_requested_size_and_stays_in_range() {
        let src = stripes(32, 8);
        let out = quilt(&src, 32, 32, 64, 48, 8, 4, 7);
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
        let a = quilt(&src, 32, 32, 64, 64, 8, 4, 1);
        let b = quilt(&src, 32, 32, 64, 64, 8, 4, 1);
        assert_eq!(a, b, "同 seed 必须逐位一致");
        let c = quilt(&src, 32, 32, 64, 64, 8, 4, 2);
        assert_ne!(a, c, "不同 seed 应给出不同拼法");
    }

    #[test]
    fn quilt_preserves_directionality_where_diffusion_does_not() {
        // 竖条纹源：合成图必须仍是大幅各向异性；随机噪声源作对照必须是各向同性
        let src = stripes(48, 8);
        let out = quilt(&src, 48, 48, 64, 64, 12, 6, 3);
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

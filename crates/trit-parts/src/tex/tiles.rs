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

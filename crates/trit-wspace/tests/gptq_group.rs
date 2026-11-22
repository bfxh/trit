//! per-group scale 版 GPTQ 的性质测试：与逐行版对齐、块内 absmean、粒度更细不变差。

use trit_wspace::gptq::{quantize, quantize_grouped};

/// xorshift64*，够用的确定性随机源（不引 rand）。
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed | 1)
    }
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    /// `[-1, 1)` 的 f32。
    fn uniform(&mut self) -> f32 {
        let bits = (self.next() >> 40) as f32 / (1u64 << 24) as f32;
        bits * 2.0 - 1.0
    }
    /// 近似标准正态（12 个均匀和减 6），用来贴近真实权重的重尾形态。
    fn gaussian(&mut self) -> f32 {
        (0..12).map(|_| self.uniform()).sum::<f32>()
    }
}

/// 高斯权重（`σ = 1` 量级），贴近真实三值化输入。
fn weight_gaussian(rows: usize, cols: usize, seed: u64) -> Vec<f32> {
    let mut rng = Rng::new(seed);
    (0..rows * cols).map(|_| rng.gaussian()).collect()
}

/// 均匀权重。
fn weight(rows: usize, cols: usize, seed: u64) -> Vec<f32> {
    let mut rng = Rng::new(seed);
    (0..rows * cols).map(|_| rng.uniform()).collect()
}

/// 由 `samples` 条随机输入算 `H = XᵀX`（正定，可直接求逆）。
///
/// 每列乘 `10^(-2..2)` 的尺度：真实激活的通道方差跨几个数量级，H 明显各向异性；
/// 各向同性时 OBS 补偿退化成 absmean（`eᵀH e ≈ ‖e‖²`），量不出 GPTQ 的收益。
fn hessian(samples: usize, cols: usize, seed: u64) -> Vec<f32> {
    let mut rng = Rng::new(seed);
    let scale: Vec<f32> = (0..cols)
        .map(|col| 10f32.powi(col as i32 % 5 - 2))
        .collect();
    let x: Vec<f32> = (0..samples * cols)
        .map(|index| rng.gaussian() * scale[index % cols])
        .collect();
    let mut h = vec![0.0f32; cols * cols];
    for row in 0..samples {
        for a in 0..cols {
            let left = x[row * cols + a];
            for b in 0..cols {
                h[a * cols + b] += left * x[row * cols + b];
            }
        }
    }
    h
}

/// `Σ_row (w − s_g·q)ᵀ H (w − s_g·q)`，`s` 按 `(row * groups + g)` 取。
fn h_error_grouped(
    weight: &[f32],
    trits: &[i8],
    scales: &[f32],
    h: &[f32],
    rows: usize,
    cols: usize,
    group: usize,
) -> f64 {
    let groups = cols.div_ceil(group);
    let mut total = 0.0f64;
    for row in 0..rows {
        let error: Vec<f64> = (0..cols)
            .map(|col| {
                f64::from(weight[row * cols + col])
                    - f64::from(scales[row * groups + col / group])
                        * f64::from(trits[row * cols + col])
            })
            .collect();
        for (a, &value) in error.iter().enumerate() {
            let mut acc = 0.0f64;
            for (b, &other) in error.iter().enumerate() {
                acc += other * f64::from(h[a * cols + b]);
            }
            total += value * acc;
        }
    }
    total
}

#[test]
fn grouped_cols_matches_per_row() {
    // group == cols ⇒ 与逐行版本逐位相同（scales 布局都是每行一个）。
    let (rows, cols) = (6, 10);
    let values = weight(rows, cols, 53);
    let h = hessian(40, cols, 59);
    let per_row = quantize(&values, rows, cols, &h).expect("quantize");
    let grouped = quantize_grouped(&values, rows, cols, &h, cols).expect("grouped");
    assert_eq!(per_row.0, grouped.0, "trits 必须相同");
    assert_eq!(per_row.1, grouped.1, "scales 必须相同");
}

#[test]
fn grouped_scale_is_block_absmean() {
    // 每组的 scale 是该组 |w| 的均值；末尾不满组按实际长度算。
    let (rows, cols, group) = (3, 7, 3);
    let values = weight(rows, cols, 61);
    let h = hessian(32, cols, 67);
    let (_, scales) = quantize_grouped(&values, rows, cols, &h, group).expect("grouped");
    let groups = cols.div_ceil(group);
    assert_eq!(scales.len(), rows * groups);
    for row in 0..rows {
        for chunk in 0..groups {
            let start = chunk * group;
            let end = (start + group).min(cols);
            let mean = values[row * cols + start..row * cols + end]
                .iter()
                .map(|value| value.abs())
                .sum::<f32>()
                / (end - start) as f32;
            assert!(
                (scales[row * groups + chunk] - mean).abs() < 1e-6,
                "第 {row} 行第 {chunk} 组的 scale 应为块内 absmean"
            );
        }
    }
}

#[test]
fn finer_group_does_not_increase_error() {
    // 更细的 scale 粒度在同一个 H 上不应变差：GPTQ 的量化点只会更贴权重。
    let (rows, cols) = (8, 16);
    let values = weight_gaussian(rows, cols, 71);
    let h = hessian(48, cols, 73);
    let coarse = quantize_grouped(&values, rows, cols, &h, cols).expect("coarse");
    let fine = quantize_grouped(&values, rows, cols, &h, 4).expect("fine");
    let coarse_err = h_error_grouped(&values, &coarse.0, &coarse.1, &h, rows, cols, cols);
    let fine_err = h_error_grouped(&values, &fine.0, &fine.1, &h, rows, cols, 4);
    assert!(
        fine_err <= coarse_err * 1.000001,
        "group=4 误差 {fine_err:.6e} 不应高于 per-row {coarse_err:.6e}"
    );
}

//! GPTQ 逐列补偿的性质测试：口径一致、对角 H 退化为 absmean、H 加权误差必下降。

use trit_wspace::gptq::{absmean, quantize};
use trit_wspace::safetensors::Weights;

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

/// `Σ_row (w − s·q)ᵀ H (w − s·q)`，f64 累加。
fn h_error(weight: &[f32], trits: &[i8], scales: &[f32], h: &[f32], cols: usize) -> f64 {
    let rows = scales.len();
    let mut total = 0.0f64;
    for row in 0..rows {
        let error: Vec<f64> = (0..cols)
            .map(|col| {
                f64::from(weight[row * cols + col])
                    - f64::from(scales[row]) * f64::from(trits[row * cols + col])
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
fn absmean_matches_safetensors_layer() {
    let (rows, cols) = (7, 13);
    let values = weight(rows, cols, 11);
    let layer = Weights {
        rows,
        cols,
        values: values.clone(),
    }
    .to_layer()
    .expect("to_layer");
    let (trits, scales) = absmean(&values, rows, cols).expect("absmean");
    for row in 0..rows {
        assert_eq!(
            scales[row],
            layer.scale(row).expect("scale"),
            "第 {row} 行 scale"
        );
        for col in 0..cols {
            assert_eq!(
                trits[row * cols + col],
                layer.trit(row, col).expect("trit").to_i8(),
                "第 {row} 行第 {col} 列"
            );
        }
    }
}

#[test]
fn diagonal_hessian_matches_absmean() {
    // 对角 H ⇒ H⁻¹ 也是对角 ⇒ 补偿项恒为 0，量化结果必须与 absmean 逐位相同。
    let (rows, cols) = (5, 9);
    let values = weight(rows, cols, 23);
    let mut h = vec![0.0f32; cols * cols];
    for index in 0..cols {
        h[index * cols + index] = 1.0 + index as f32;
    }
    let base = absmean(&values, rows, cols).expect("absmean");
    let gptq = quantize(&values, rows, cols, &h).expect("quantize");
    assert_eq!(base.0, gptq.0, "对角 H 下 trits 必须与 absmean 相同");
    assert_eq!(base.1, gptq.1, "scale 口径必须一致");
}

#[test]
fn gptq_reduces_hessian_error() {
    // 校准集上 GPTQ 的 H 加权误差必须严格小于 absmean（补偿方向正确）。
    let (rows, cols) = (12, 16);
    let values = weight_gaussian(rows, cols, 31);
    let h = hessian(64, cols, 37);
    let base = absmean(&values, rows, cols).expect("absmean");
    let gptq = quantize(&values, rows, cols, &h).expect("quantize");
    let base_err = h_error(&values, &base.0, &base.1, &h, cols);
    let gptq_err = h_error(&values, &gptq.0, &gptq.1, &h, cols);
    // 合成权重与 H 相互独立，补偿只能吃到有限收益（实测 −9%）；真实 MiniLM 上同一
    // 算法把 H 加权误差降 37%。这里的门槛是"方向必须对"：补偿系数误用 H 而不是 H⁻¹
    // 时误差会涨到 3 倍，这条断言立刻红。
    assert!(
        gptq_err < base_err * 0.999,
        "GPTQ 误差 {gptq_err:.6e} 必须低于 absmean {base_err:.6e}"
    );
    // trits 必须仍是三值，scale 必须是有限正数。
    assert!(gptq.0.iter().all(|trit| (-1..=1).contains(trit)));
    assert!(gptq.1.iter().all(|scale| scale.is_finite() && *scale > 0.0));
}

#[test]
fn quantize_is_deterministic() {
    let (rows, cols) = (4, 6);
    let values = weight(rows, cols, 43);
    let h = hessian(32, cols, 47);
    let first = quantize(&values, rows, cols, &h).expect("quantize");
    let second = quantize(&values, rows, cols, &h).expect("quantize");
    assert_eq!(first.0, second.0);
    assert_eq!(first.1, second.1);
}

#[test]
fn shape_errors_are_reported() {
    assert!(absmean(&[0.0; 5], 2, 3).is_err(), "weight 长度不符必须报错");
    assert!(
        quantize(&[0.1; 6], 2, 3, &[0.0; 8]).is_err(),
        "H 维度不符必须报错"
    );
    assert!(quantize(&[0.1; 6], 2, 0, &[]).is_err(), "零列必须报错");
}

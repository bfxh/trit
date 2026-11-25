//! 低秩残差侧车的性质：满秩重构、截断单调、两种取向一致、参数计数。

use trit_wspace::gptq::absmean;
use trit_wspace::residual::LowRankResidual;

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
    fn uniform(&mut self) -> f32 {
        let bits = (self.next() >> 40) as f32 / (1u64 << 24) as f32;
        bits * 2.0 - 1.0
    }
    /// 近似标准正态（12 个均匀和减 6），贴近真实权重的重尾形态。
    fn gaussian(&mut self) -> f32 {
        (0..12).map(|_| self.uniform()).sum::<f32>()
    }
}

fn weight(rows: usize, cols: usize, seed: u64) -> Vec<f32> {
    let mut rng = Rng::new(seed);
    (0..rows * cols).map(|_| rng.gaussian()).collect()
}

/// `R = W − s·T`（与 `LowRankResidual::build` 内同一口径）。
fn residual(
    values: &[f32],
    rows: usize,
    cols: usize,
    trits: &[i8],
    scales: &[f32],
    group: usize,
) -> Vec<f64> {
    let groups = cols.div_ceil(group);
    (0..rows * cols)
        .map(|index| {
            let row = index / cols;
            let col = index % cols;
            f64::from(values[index])
                - f64::from(trits[index]) * f64::from(scales[row * groups + col / group])
        })
        .collect()
}

/// 用 e_j 作输入逐列取回 rank 截断后的近似矩阵。
fn approximation(sidecar: &LowRankResidual, cols: usize) -> Vec<f64> {
    let mut out = vec![0.0f64; sidecar.rows() * cols];
    for col in 0..cols {
        let mut unit = vec![0.0f32; cols];
        unit[col] = 1.0;
        for row in 0..sidecar.rows() {
            out[row * cols + col] = sidecar.correction_direct(row, &unit);
        }
    }
    out
}

fn squared_error(left: &[f64], right: &[f64]) -> f64 {
    left.iter()
        .zip(right)
        .map(|(&a, &b)| (a - b) * (a - b))
        .sum()
}

/// 每行按 `group` 列切块、块内 absmean 的三值化（与 `QuantLinear` 的 per-group 口径一致）。
fn grouped_quant(values: &[f32], rows: usize, cols: usize, group: usize) -> (Vec<i8>, Vec<f32>) {
    let groups = cols.div_ceil(group);
    let mut trits = vec![0i8; rows * cols];
    let mut scales = vec![0.0f32; rows * groups];
    for row in 0..rows {
        for chunk in 0..groups {
            let start = chunk * group;
            let end = (start + group).min(cols);
            let mean = values[row * cols + start..row * cols + end]
                .iter()
                .map(|value| value.abs())
                .sum::<f32>()
                / (end - start) as f32;
            let scale = if mean == 0.0 { 1.0 } else { mean };
            scales[row * groups + chunk] = scale;
            for col in start..end {
                trits[row * cols + col] =
                    (values[row * cols + col] / scale).round().clamp(-1.0, 1.0) as i8;
            }
        }
    }
    (trits, scales)
}

#[test]
fn full_rank_reproduces_residual_for_tall_matrix() {
    let (rows, cols, group) = (12, 7, 3);
    let values = weight(rows, cols, 101);
    let (trits, scales) = grouped_quant(&values, rows, cols, group);
    let sidecar = LowRankResidual::build(&values, rows, cols, &trits, &scales, group, cols)
        .expect("build")
        .expect("rank > 0");
    assert_eq!(sidecar.rank(), cols);
    assert_eq!(sidecar.params(), cols * (rows + cols));

    let exact = residual(&values, rows, cols, &trits, &scales, group);
    let approx = approximation(&sidecar, cols);
    let error = squared_error(&approx, &exact);
    let norm = exact.iter().map(|value| value * value).sum::<f64>();
    assert!(
        error < 1e-8 * norm.max(1.0),
        "满秩误差 {error} 过大（残差范数² {norm}）"
    );
}

#[test]
fn full_rank_reproduces_residual_for_wide_matrix() {
    let (rows, cols, group) = (5, 13, 4);
    let values = weight(rows, cols, 107);
    let (trits, scales) = grouped_quant(&values, rows, cols, group);
    let sidecar = LowRankResidual::build(&values, rows, cols, &trits, &scales, group, cols)
        .expect("build")
        .expect("rank > 0");
    assert_eq!(sidecar.rank(), rows, "rank 上限是 min(rows, cols)");

    let exact = residual(&values, rows, cols, &trits, &scales, group);
    let approx = approximation(&sidecar, cols);
    let error = squared_error(&approx, &exact);
    let norm = exact.iter().map(|value| value * value).sum::<f64>();
    assert!(
        error < 1e-8 * norm.max(1.0),
        "宽矩阵满秩误差 {error} 过大（{norm}）"
    );
}

#[test]
fn truncation_error_is_monotone_in_rank() {
    let (rows, cols, group) = (16, 9, 3);
    let values = weight(rows, cols, 109);
    let (trits, scales) = grouped_quant(&values, rows, cols, group);
    let exact = residual(&values, rows, cols, &trits, &scales, group);
    let mut previous = f64::INFINITY;
    for rank in 1..=rows.min(cols) {
        let sidecar = LowRankResidual::build(&values, rows, cols, &trits, &scales, group, rank)
            .expect("build")
            .expect("rank > 0");
        let error = squared_error(&approximation(&sidecar, cols), &exact);
        assert!(
            error <= previous * 1.0000001,
            "rank {rank} 误差 {error} 大于前一个 {previous}"
        );
        previous = error;
    }
}

#[test]
fn set_rank_only_shrinks_and_keeps_prefix() {
    let (rows, cols) = (6, 4);
    let values = weight(rows, cols, 113);
    let (trits, scales) = absmean(&values, rows, cols).expect("absmean");
    let mut sidecar = LowRankResidual::build(&values, rows, cols, &trits, &scales, cols, 4)
        .expect("build")
        .expect("rank > 0");
    let full = approximation(&sidecar, cols);
    assert!(sidecar.set_rank(2).is_ok());
    let truncated = approximation(&sidecar, cols);
    assert_eq!(sidecar.params(), 2 * (rows + cols));
    // 前缀截断只能减误差，不能超过满秩近似。
    let exact = residual(&values, rows, cols, &trits, &scales, cols);
    assert!(
        squared_error(&truncated, &exact) >= squared_error(&full, &exact) - 1e-9,
        "截断后误差不应低于满秩"
    );
    assert!(
        sidecar.set_rank(5).is_err(),
        "rank 不能超过构造时的 capacity"
    );
}

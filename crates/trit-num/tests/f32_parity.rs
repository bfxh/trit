//! f64 路径 vs f32 参考实现的同题对拍（固定种子的合成数据）。

use trit_num::{ols_f32, ols_f64};

/// 固定种子的 SplitMix64（与生产代码无关，测试内自建）。
struct Rng {
    state: u64,
}

impl Rng {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_f64(&mut self) -> f64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        let bits = z ^ (z >> 31);
        // [0, 1) 的均匀值
        (bits >> 11) as f64 / (1u64 << 53) as f64
    }

    fn uniform(&mut self, low: f64, high: f64) -> f64 {
        low + (high - low) * self.next_f64()
    }
}

fn synthetic(n: usize, noise: f64, seed: u64) -> (Vec<Vec<f64>>, Vec<f64>, [f64; 4]) {
    let truth = [1.5, 0.5, -1.25, 2.0]; // 截距 + 3 个斜率
    let mut rng = Rng::new(seed);
    let mut x = Vec::with_capacity(n);
    let mut y = Vec::with_capacity(n);
    for _ in 0..n {
        let row = vec![
            rng.uniform(-2.0, 2.0),
            rng.uniform(-2.0, 2.0),
            rng.uniform(-2.0, 2.0),
        ];
        let noise_term = rng.uniform(-noise, noise);
        let value =
            truth[0] + truth[1] * row[0] + truth[2] * row[1] + truth[3] * row[2] + noise_term;
        x.push(row);
        y.push(value);
    }
    (x, y, truth)
}

#[test]
fn f64_matches_f32_reference_on_well_conditioned_problem() {
    let (x, y, _) = synthetic(128, 1e-3, 0x5EED);
    let fit = ols_f64(&x, &y, 0.0).unwrap();
    let beta32 = ols_f32(&x, &y, 0.0).unwrap();
    assert_eq!(beta32.len(), fit.coefficients.len() + 1);
    let reference = std::iter::once(fit.intercept)
        .chain(fit.coefficients.iter().copied())
        .collect::<Vec<f64>>();
    for (index, (a, b)) in reference.iter().zip(&beta32).enumerate() {
        let scale = a.abs().max(1.0);
        let relative = (a - f64::from(*b)).abs() / scale;
        assert!(relative < 1e-4, "coefficient {index}: f64={a} f32={b}");
    }
}

#[test]
fn f64_recovers_truth_better_than_f32() {
    let (x, y, truth) = synthetic(256, 1e-6, 42);
    let fit = ols_f64(&x, &y, 0.0).unwrap();
    let beta32 = ols_f32(&x, &y, 0.0).unwrap();
    let f64_error = (fit.intercept - truth[0]).abs()
        + fit
            .coefficients
            .iter()
            .zip(&truth[1..])
            .map(|(c, t)| (c - t).abs())
            .sum::<f64>();
    let f32_error = (f64::from(beta32[0]) - truth[0]).abs()
        + beta32[1..]
            .iter()
            .zip(&truth[1..])
            .map(|(c, t)| (f64::from(*c) - t).abs())
            .sum::<f64>();
    // 噪声 1e-6、n=256 ⇒ 参数误差量级约 1e-7；这里只要求 f64 明显好于 f32。
    assert!(f64_error < 1e-6, "f64 误差 {f64_error}");
    assert!(
        f64_error < f32_error,
        "f64 误差 {f64_error} 应小于 f32 误差 {f32_error}"
    );
}

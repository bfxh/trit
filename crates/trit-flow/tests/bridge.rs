//! 桥的对拍：`x̂₀ ↔ v` 两条公式必须互逆，且与扩散的定义式逐项一致。
//!
//! 定义：`x = √ᾱ·x̂₀ + √(1−ᾱ)·ε̂`、`v = √ᾱ·ε̂ − √(1−ᾱ)·x̂₀`。
//! 本 crate 用 `v = (√ᾱ·x − x̂₀)/√(1−ᾱ)`——这条测试证明两者代数等价（不是近似）。

use trit_flow::{
    Constant, Scheme, Scratch, X0Predictor, X0ToVelocity, advance, velocity_to_x0, x0_to_velocity,
};

fn abar_table(steps: usize) -> Vec<f32> {
    // 余弦表（与 trit-parts::diffuse::Schedule::cosine 同口径）
    let s = 0.008f32;
    let f = |u: f32| {
        ((u + s) / (1.0 + s) * std::f32::consts::FRAC_PI_2)
            .cos()
            .powi(2)
    };
    let f0 = f(0.0);
    (0..=steps)
        .map(|i| (f(i as f32 / steps as f32) / f0).clamp(0.0, 1.0))
        .collect()
}

#[test]
fn velocity_and_x0_are_inverse() {
    // 覆盖低/中/高 t 三段
    for &a in &[0.999f32, 0.9, 0.5, 0.1, 0.01] {
        for (x0, eps) in [(0.2f32, -1.3f32), (0.7, 0.4), (-0.1, 2.0)] {
            let sa = a.sqrt();
            let se = (1.0 - a).sqrt();
            let x = sa * x0 + se * eps;
            let v_def = sa * eps - se * x0; // 直接由定义算
            let v = x0_to_velocity(a, x, x0);
            assert!(
                (v - v_def).abs() < 2e-5,
                "a={a}：桥 {v} 与定义 {v_def} 不一致"
            );
            let back = velocity_to_x0(a, x, v);
            assert!((back - x0).abs() < 2e-5, "a={a}：v→x̂₀ 往返 {back} ≠ {x0}");
        }
    }
}

#[test]
fn bridge_is_stable_at_high_t() {
    // 高 t（ᾱ → 0）：v ≈ −x̂₀，没有 1/√ᾱ 放大
    let a = 1e-4f32;
    let x0 = 0.3f32;
    let eps = 0.9f32;
    let x = a.sqrt() * x0 + (1.0 - a).sqrt() * eps;
    let v = x0_to_velocity(a, x, x0);
    assert!((v + x0).abs() < 1e-2, "高 t 时 v 应 ≈ −x̂₀，实测 {v}");
    assert!(v.is_finite());
}

/// 常数 x̂₀ 预测器（理想去噪器）。
struct ConstX0 {
    dim: usize,
    value: f32,
}

impl X0Predictor for ConstX0 {
    fn dim(&self) -> usize {
        self.dim
    }
    fn predict_x0(&self, _x: &[f32], _t: &[f32], _cond: &[f32], out: &mut [f32]) {
        for o in out.iter_mut() {
            *o = self.value;
        }
    }
}

#[test]
fn euler_on_bridge_converges_to_ddim_chain() {
    // 同一常数 x̂₀ = 0.25 的两条路：
    // - DDIM 闭式链（表档数 T 步，ε 空间逐步反推 x̂₀）
    // - 桥 + Euler（同 T 步）
    // 两者都应落在 0.25；差值只来自 Euler 的一阶离散误差。表与步数取同一档数，
    // 避免 ᾱ 量化（t 很小时表被 round 到 idx 0 = 1）混进读数。
    let t_steps = 1024;
    let abar = abar_table(t_steps);
    let x0 = 0.25f32;

    let mut x = 0.8f32; // 从噪声侧出发
    for t in (1..=t_steps).rev() {
        let a_t = abar[t];
        let a_prev = abar[t - 1];
        let eps = (x - a_t.sqrt() * x0) / (1.0 - a_t).sqrt();
        x = a_prev.sqrt() * x0 + (1.0 - a_prev).sqrt() * eps;
    }
    let ddim_end = x;

    let bridge = X0ToVelocity::new(ConstX0 { dim: 1, value: x0 }, abar);
    let mut x = vec![0.8f32];
    let mut scratch = Scratch::new();
    let n = t_steps;
    for k in 0..n {
        let t = 1.0 - k as f32 / n as f32;
        advance(
            &bridge,
            Scheme::Euler,
            &mut x,
            &[t],
            &[-1.0 / n as f32],
            &[],
            &mut scratch,
        );
    }
    let euler_end = x[0];

    assert!(
        (ddim_end - x0).abs() < 5e-3,
        "DDIM 链终点 {ddim_end} 应回到 x̂₀ = {x0}"
    );
    assert!(
        (euler_end - x0).abs() < 5e-2,
        "Euler 终点 {euler_end} 应在 x̂₀ 附近"
    );
    assert!(
        (euler_end - ddim_end).abs() < 5e-2,
        "两条路终点应接近：Euler {euler_end} vs DDIM {ddim_end}"
    );
}

#[test]
fn bridge_output_stays_finite_away_from_t_zero() {
    let abar = abar_table(16);
    let bridge = X0ToVelocity::new(ConstX0 { dim: 3, value: 0.5 }, abar);
    let mut x = vec![0.1f32; 3]; // 3 维 = 1 个样本
    let mut scratch = Scratch::new();
    for k in 0..16 {
        let t = 1.0 - k as f32 / 16.0;
        advance(
            &bridge,
            Scheme::Euler,
            &mut x,
            &[t],
            &[-1.0 / 16.0],
            &[],
            &mut scratch,
        );
    }
    for v in &x {
        assert!(v.is_finite(), "积分产生非有限值：{v}");
    }
}

#[test]
fn plain_constant_is_untouched_by_bridge_type() {
    // 不加桥的常数场作为对照：一步 = v·Δt
    let f = Constant { v: vec![-0.5; 2] };
    let mut x = vec![1.0f32; 2]; // 2 维 = 1 个样本
    let mut scratch = Scratch::new();
    advance(
        &f,
        Scheme::Euler,
        &mut x,
        &[1.0],
        &[-0.2],
        &[],
        &mut scratch,
    );
    for v in x {
        assert!(
            (v - 1.1).abs() < 1e-6,
            "1.0 + (−0.5)×(−0.2) = 1.1，实测 {v}"
        );
    }
    assert_eq!(f.v.len(), 2);
}

//! 末步 snap：扩散侧按 DDIM 惯例直接读 `x̂₀`，这是 `t→0` stiff 的修法。
//!
//! 判据两条：
//! 1. 打开 snap，扩散侧终点**精确**等于去噪器给的值（不再被 `1/√(1−ᾱ)` 放大）；
//! 2. 纯速度场不受影响（`terminal_snap` 默认返回 `false`，开关不改变结果）。

use std::sync::Arc;

use trit_flow::{Cfg, Executor, Field, MlpField, Request, Scheme, X0Predictor, X0ToVelocity};

fn abar_table(steps: usize) -> Vec<f32> {
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

fn run(field: Arc<dyn Field + Send + Sync>, req: &Request, snap: bool) -> Vec<f32> {
    let ex = Executor::new(
        field,
        Cfg {
            threads: 0,
            max_batch: 4,
            snap,
        },
    );
    let (out, _) = ex.run(std::slice::from_ref(req)).expect("单请求");
    out[0].x.clone()
}

#[test]
fn snap_makes_diffusion_endpoint_exact() {
    let steps = 32;
    let dim = 4;
    let bridge: Arc<dyn Field + Send + Sync> = Arc::new(X0ToVelocity::new(
        ConstX0 { dim, value: 0.25 },
        abar_table(steps),
    ));
    let req = Request::new(vec![0.8; dim], steps, Scheme::Euler);

    let with = run(Arc::clone(&bridge), &req, true);
    let without = run(bridge, &req, false);

    for (j, v) in with.iter().enumerate() {
        assert!(
            (v - 0.25).abs() < 1e-6,
            "第 {j} 维：snap 后应精确等于 x̂₀ = 0.25，实测 {v}"
        );
    }
    // 不 snap 时 32 步一阶 Euler 有明显残差（实测约 0.061，即 24% 相对误差）；
    // snap 应把它压到千分之一以内。
    let e_with = (with[0] - 0.25).abs();
    let e_without = (without[0] - 0.25).abs();
    assert!(
        e_with * 1000.0 < e_without,
        "snap 应把端点误差压到无 snap 的 1/1000 以内：snap {e_with:.3e}，无 snap {e_without:.3e}"
    );
}

#[test]
fn snap_is_opt_in_for_pure_velocity_fields() {
    // 纯速度场没有末步替换：开关前后逐位一致
    let f: Arc<dyn Field + Send + Sync> = Arc::new(MlpField::new(8, 0, 6, 3));
    let req = Request::new(vec![0.5; 8], 5, Scheme::Euler);
    let a = run(Arc::clone(&f), &req, true);
    let b = run(f, &req, false);
    assert_eq!(a, b, "纯速度场不应受 snap 开关影响");
}

#[test]
fn snap_respects_condition() {
    // 条件不同 ⇒ 终点的 x̂₀ 不同（snap 走的是带条件的预测器）
    struct CondX0;
    impl X0Predictor for CondX0 {
        fn dim(&self) -> usize {
            2
        }
        fn cond_dim(&self) -> usize {
            1
        }
        fn predict_x0(&self, _x: &[f32], _t: &[f32], cond: &[f32], out: &mut [f32]) {
            for b in 0..cond.len() {
                out[2 * b] = cond[b];
                out[2 * b + 1] = -cond[b];
            }
        }
    }
    let steps = 16;
    let bridge: Arc<dyn Field + Send + Sync> =
        Arc::new(X0ToVelocity::new(CondX0, abar_table(steps)));
    let req = Request::with_cond(vec![0.3; 2], vec![0.75], steps, Scheme::Euler);
    let out = run(bridge, &req, true);
    assert!(
        (out[0] - 0.75).abs() < 1e-6,
        "条件通道 0 应为 0.75，实测 {}",
        out[0]
    );
    assert!(
        (out[1] + 0.75).abs() < 1e-6,
        "条件通道 1 应为 −0.75，实测 {}",
        out[1]
    );
}

#[test]
fn snap_keeps_batch_parity() {
    // 批 + 并行下，末步 snap 也必须是逐位一致的
    let steps = 24;
    let bridge: Arc<dyn Field + Send + Sync> = Arc::new(X0ToVelocity::new(
        ConstX0 { dim: 4, value: 0.4 },
        abar_table(steps),
    ));
    let reqs: Vec<Request> = (0..5)
        .map(|i| Request::new(vec![0.1 + 0.1 * i as f32; 4], steps, Scheme::Heun))
        .collect();
    let par = Executor::new(
        Arc::clone(&bridge),
        Cfg {
            threads: 4,
            max_batch: 8,
            snap: true,
        },
    );
    let (a, _) = par.run(&reqs).expect("并行 + snap");
    for (i, r) in reqs.iter().enumerate() {
        let single = run(Arc::clone(&bridge), r, true);
        assert_eq!(a[i].x, single, "请求 {i}：批 + 并行 + snap 与单请求不一致");
    }
}

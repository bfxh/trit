//! 积分器：把任意 [`Field`] 沿时间从噪声推到数据。
//!
//! 三种方案都只依赖「求速度」这一个原语，所以扩散与流匹配共用：
//!
//! - [`Scheme::Euler`]：`x ← x + Δt·v(x,t)`——DDIM（η=0）在这个参数化下**就是**它；
//! - [`Scheme::Midpoint`]、[`Scheme::Heun`]：每步两次求值，二阶（Heun 即梯形法）。
//!
//! 步长按样本给（`dt[b]`），因此**不同步数的请求能在同一批里推进**：
//! 请求 A 走 1/50、请求 B 走 1/100，同一次 `eval` 调用里各自用自己的 `t` 与 `Δt`。

use crate::field::Field;

/// 积分方案。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scheme {
    /// 一阶显式欧拉（等价于 DDIM η=0 的确定性更新）。
    Euler,
    /// 二阶中点法。
    Midpoint,
    /// 二阶 Heun（梯形法）。
    Heun,
}

impl Scheme {
    /// 每步求值次数。
    #[must_use]
    pub fn evals_per_step(self) -> usize {
        match self {
            Self::Euler => 1,
            Self::Midpoint | Self::Heun => 2,
        }
    }
}

/// 一次采样的方案 + 步数。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Solver {
    /// 积分方案。
    pub scheme: Scheme,
    /// 步数（≥ 1）。
    pub steps: usize,
}

impl Solver {
    /// 建方案。
    ///
    /// # Panics
    /// `steps == 0`。
    #[must_use]
    pub fn new(scheme: Scheme, steps: usize) -> Self {
        assert!(steps > 0, "步数必须 ≥ 1");
        Self { scheme, steps }
    }

    /// 从噪声到数据的等距时间网格，长度 `steps + 1`（含 1.0 与 0.0）。
    #[must_use]
    pub fn times(&self) -> Vec<f32> {
        let n = self.steps;
        (0..=n).map(|k| 1.0 - k as f32 / n as f32).collect()
    }

    /// 单步步长（负值：时间从 1 递减到 0）。
    #[must_use]
    pub fn dt(&self) -> f32 {
        -1.0 / self.steps as f32
    }
}

/// `advance` 的临时缓冲（按批大小复用，避免每步分配）。
#[derive(Debug, Clone, Default)]
pub struct Scratch {
    k1: Vec<f32>,
    k2: Vec<f32>,
    xm: Vec<f32>,
    tm: Vec<f32>,
}

impl Scratch {
    /// 建空缓冲。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 保证缓冲容得下 `n × dim` 的一批。
    pub fn ensure(&mut self, n: usize, dim: usize) {
        let len = n * dim;
        if self.k1.len() < len {
            self.k1.resize(len, 0.0);
            self.k2.resize(len, 0.0);
            self.xm.resize(len, 0.0);
        }
        if self.tm.len() < n {
            self.tm.resize(n, 0.0);
        }
    }

    /// 缓存的批容量（样本数 × 维数）。
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.k1.len()
    }
}

/// 把一批样本推进**一步**。
///
/// - `x`：`n × dim` 行主序，原地更新；
/// - `t`：每样本当前时间（0 数据 … 1 噪声），长度 `n`；
/// - `dt`：每样本步长（负值），长度 `n`；
/// - `cond`：`n × cond_dim`（无条件时空切片）。
///
/// # Panics
/// 形状不合法（调用方应先用 [`crate::field::check_batch`] 校验）。
pub fn advance(
    field: &dyn Field,
    scheme: Scheme,
    x: &mut [f32],
    t: &[f32],
    dt: &[f32],
    cond: &[f32],
    scratch: &mut Scratch,
) {
    let dim = field.dim();
    let n = t.len();
    assert_eq!(x.len(), n * dim, "x 长度必须是 n×dim");
    assert_eq!(dt.len(), n, "dt 长度必须是 n");
    assert_eq!(
        cond.len(),
        n * field.cond_dim(),
        "cond 长度必须是 n×cond_dim"
    );
    scratch.ensure(n, dim);

    let Scratch { k1, k2, xm, tm } = scratch;
    let k1 = &mut k1[..n * dim];
    let k2 = &mut k2[..n * dim];
    let xm = &mut xm[..n * dim];
    let tm = &mut tm[..n];

    match scheme {
        Scheme::Euler => {
            field.eval(x, t, cond, k1);
            for i in 0..x.len() {
                x[i] += dt[i / dim] * k1[i];
            }
        }
        Scheme::Midpoint => {
            field.eval(x, t, cond, k1);
            for i in 0..x.len() {
                xm[i] = x[i] + 0.5 * dt[i / dim] * k1[i];
            }
            for b in 0..n {
                tm[b] = t[b] + 0.5 * dt[b];
            }
            field.eval(xm, tm, cond, k2);
            for i in 0..x.len() {
                x[i] += dt[i / dim] * k2[i];
            }
        }
        Scheme::Heun => {
            field.eval(x, t, cond, k1);
            for i in 0..x.len() {
                xm[i] = x[i] + dt[i / dim] * k1[i];
            }
            for b in 0..n {
                tm[b] = t[b] + dt[b];
            }
            field.eval(xm, tm, cond, k2);
            for i in 0..x.len() {
                x[i] += 0.5 * dt[i / dim] * (k1[i] + k2[i]);
            }
        }
    }
}

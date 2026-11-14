//! 统一速度场：生成模型的一步 = 在状态空间里求一个速度。
//!
//! 概率流 ODE 把两类主流生成模型写成同一条式子（`t` 从 1 噪声到 0 数据）：
//!
//! ```text
//! dx/dt = v(x, t)
//! ```
//!
//! - **流匹配 / 漂移场**（flow matching、rectified flow、SDE 的漂移项）：网络直接输出 `v`；
//! - **扩散**（DDIM η=0 的确定性采样）：网络输出 `x̂₀`，换算成同一个 `v`
//!   （见 [`X0ToVelocity`]：`v = (√ᾱ·x − x̂₀)/√(1−ᾱ)`）。
//!
//! 于是「换模型」只剩换一个 [`Field`] 实现——积分器、批量执行器与读数层都不动。
//! 这是本 crate 存在的理由：**采样骨架与模型解耦，任何能给出速度的场都能接上来**。

use trit_bench::SeedRng;

/// 批量求值的速度场。
///
/// 口径（实现方必须遵守，调用方由 [`check_batch`] 保证输入合法）：
/// - `x`、`out` 都是 `batch * dim` 行主序，第 `b` 个样本占 `[b*dim, (b+1)*dim)`；
/// - `t[b]` 是第 `b` 个样本**自己的**时间（0 数据 … 1 噪声），批内可以各不相同
///   ——不同步数、不同阶段的请求因此能同批推进；
/// - `cond` 是 `batch * cond_dim` 行主序（`cond_dim = 0` 时为空切片）；
/// - 写 `out`，不改 `x`。
///
/// `t = 0` 端点不要求可求值（扩散桥在 `ᾱ = 1` 处除以 `√(1−ᾱ)`）；积分到 `t = 0`
/// 之后不再求值，这一点由积分器保证。
pub trait Field {
    /// 状态维数（每样本多少个 f32）。
    fn dim(&self) -> usize;
    /// 条件维数（每样本多少个 f32）；默认无条件。
    fn cond_dim(&self) -> usize {
        0
    }
    /// 批量求速度，写进 `out`。
    fn eval(&self, x: &[f32], t: &[f32], cond: &[f32], out: &mut [f32]);

    /// 末端 snap（可选）：积分**最后一步之后**用它替换结果。
    ///
    /// 扩散侧为什么要它：`v = (√ᾱ·x − x̂₀)/√(1−ᾱ)` 在 `t→0` 被 `1/√(1−ᾱ)` 放大，
    /// 继续积分只会放大误差；DDIM 的惯例是末步直接读去噪器（`x ← x̂₀`）。
    ///
    /// 返回 `true` 表示 `out`（长度同 `x`）已写入数据侧估计；默认 `false` = 不替换，
    /// 纯速度场（流匹配、任意自定义场）什么都不用做。
    fn terminal_snap(&self, _x: &[f32], _t: &[f32], _cond: &[f32], _out: &mut [f32]) -> bool {
        false
    }
}

/// 批形状不合法。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShapeError {
    /// 哪一处不合法。
    pub what: &'static str,
    /// 实际长度。
    pub got: usize,
    /// 期望长度。
    pub want: usize,
}

impl std::fmt::Display for ShapeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: 长度 {}，期望 {}（batch×dim 口径）",
            self.what, self.got, self.want
        )
    }
}

impl std::error::Error for ShapeError {}

/// 校验一批输入的形状，返回批大小。
pub fn check_batch(
    dim: usize,
    cond_dim: usize,
    x: &[f32],
    t: &[f32],
    cond: &[f32],
    out: &[f32],
) -> Result<usize, ShapeError> {
    if dim == 0 {
        return Err(ShapeError {
            what: "场维数为 0",
            got: 0,
            want: 1,
        });
    }
    if x.len() != out.len() {
        return Err(ShapeError {
            what: "out 与 x",
            got: out.len(),
            want: x.len(),
        });
    }
    if !x.len().is_multiple_of(dim) {
        return Err(ShapeError {
            what: "x 不是 dim 的整数倍",
            got: x.len(),
            want: dim,
        });
    }
    let n = x.len() / dim;
    if t.len() != n {
        return Err(ShapeError {
            what: "t 的样本数",
            got: t.len(),
            want: n,
        });
    }
    if cond.len() != n * cond_dim {
        return Err(ShapeError {
            what: "cond 的样本数×条件维",
            got: cond.len(),
            want: n * cond_dim,
        });
    }
    Ok(n)
}

/// 查 `ᾱ(t)`：与 `trit-parts::tconv::diffusion::VDen` 同一口径（`idx = round(t·T)`，夹到表内）。
///
/// # Panics
/// `abar` 为空。
#[must_use]
pub fn abar_at(abar: &[f32], t: f32) -> f32 {
    assert!(!abar.is_empty(), "ᾱ 表为空");
    let tmax = abar.len() - 1;
    let idx = ((t.clamp(0.0, 1.0) * tmax as f32).round() as usize).min(tmax);
    abar[idx]
}

/// `x̂₀ → v`：`v = (√ᾱ·x − x̂₀)/√(1−ᾱ)`。
///
/// 与 `x = √ᾱ·x̂₀ + √(1−ᾱ)·ε̂`、`v = √ᾱ·ε̂ − √(1−ᾱ)·x̂₀` 逐项等价（见 `tests`）。
#[must_use]
pub fn x0_to_velocity(a: f32, x: f32, x0: f32) -> f32 {
    (a.sqrt() * x - x0) / (1.0 - a).sqrt()
}

/// `v → x̂₀`：`x̂₀ = √ᾱ·x − √(1−ᾱ)·v`（高 `t` 时没有 `1/√ᾱ`，这就是 v-预测稳定的原因）。
#[must_use]
pub fn velocity_to_x0(a: f32, x: f32, v: f32) -> f32 {
    a.sqrt() * x - (1.0 - a).sqrt() * v
}

/// 常数速度场（解析 oracle：一步就是精确解）。
#[derive(Debug, Clone, PartialEq)]
pub struct Constant {
    /// 速度，长度 = `dim`。
    pub v: Vec<f32>,
}

impl Field for Constant {
    fn dim(&self) -> usize {
        self.v.len()
    }
    fn eval(&self, _x: &[f32], _t: &[f32], _cond: &[f32], out: &mut [f32]) {
        for (i, o) in out.iter_mut().enumerate() {
            *o = self.v[i % self.v.len()];
        }
    }
}

/// 对角线性漂移场：`v = a ⊙ (x − b)`，逐维独立 ⇒ 闭式解
/// `x(t) = b + (x₀ − b)·e^{a·(t − t₀)}` 可用来测积分阶数。
#[derive(Debug, Clone, PartialEq)]
pub struct LinearDrift {
    /// 每个维度的衰减率（负值 = 向 `b` 收敛）。
    pub a: Vec<f32>,
    /// 不动点。
    pub b: Vec<f32>,
}

impl Field for LinearDrift {
    fn dim(&self) -> usize {
        self.a.len()
    }
    fn eval(&self, x: &[f32], _t: &[f32], _cond: &[f32], out: &mut [f32]) {
        for (i, o) in out.iter_mut().enumerate() {
            let d = i % self.a.len();
            *o = self.a[d] * (x[i] - self.b[d]);
        }
    }
}

/// 闭包场：把任意既有模型（含三值网络、注意力、外部推理）直接接进积分器。
pub struct FnField<F> {
    /// 状态维数。
    pub dim: usize,
    /// 条件维数。
    pub cond_dim: usize,
    /// 求值函数，签名与 [`Field::eval`] 一致。
    pub f: F,
}

impl<F> Field for FnField<F>
where
    F: Fn(&[f32], &[f32], &[f32], &mut [f32]),
{
    fn dim(&self) -> usize {
        self.dim
    }
    fn cond_dim(&self) -> usize {
        self.cond_dim
    }
    fn eval(&self, x: &[f32], t: &[f32], cond: &[f32], out: &mut [f32]) {
        (self.f)(x, t, cond, out);
    }
}

/// 扩散侧的「输出 x̂₀」预测器（批量）。
pub trait X0Predictor {
    /// 状态维数。
    fn dim(&self) -> usize;
    /// 条件维数。
    fn cond_dim(&self) -> usize {
        0
    }
    /// 批量预测 `x̂₀`，写进 `out`（布局同 [`Field::eval`]）。
    fn predict_x0(&self, x: &[f32], t: &[f32], cond: &[f32], out: &mut [f32]);
}

/// 把「预测 x̂₀」的扩散去噪器包装成统一速度场。
///
/// `v = (√ᾱ·x − x̂₀)/√(1−ᾱ)`：写开就是 `v = √ᾱ·ε̂ − √(1−ᾱ)·x̂₀`，
/// 所以 DDIM（η=0）与流匹配的 Euler 积分**是同一个更新**，只是参数化不同。
pub struct X0ToVelocity<P> {
    /// 去噪器。
    pub pred: P,
    /// 余弦（或任意）噪声表，长度 `T+1`。
    pub abar: Vec<f32>,
}

impl<P: X0Predictor> X0ToVelocity<P> {
    /// 建桥。
    #[must_use]
    pub fn new(pred: P, abar: Vec<f32>) -> Self {
        assert!(!abar.is_empty(), "ᾱ 表为空");
        Self { pred, abar }
    }
}

impl<P: X0Predictor> Field for X0ToVelocity<P> {
    /// 末步直接取 `x̂₀`（DDIM 惯例）：这是扩散侧 `t→0` stiff 的修法。
    fn terminal_snap(&self, x: &[f32], t: &[f32], cond: &[f32], out: &mut [f32]) -> bool {
        self.pred.predict_x0(x, t, cond, out);
        true
    }

    fn dim(&self) -> usize {
        self.pred.dim()
    }
    fn cond_dim(&self) -> usize {
        self.pred.cond_dim()
    }
    fn eval(&self, x: &[f32], t: &[f32], cond: &[f32], out: &mut [f32]) {
        self.pred.predict_x0(x, t, cond, out);
        let dim = self.pred.dim();
        for i in 0..x.len() {
            // 表在 t→0 端会量化到 ᾱ = 1，此时 1/√(1−ᾱ) 是奇点：按 1−1e-6 截断
            // （误差 O(1e-3)，与 VDen 的 clamp 同一性质，不是静默近似——见 crate 文档）。
            let a = abar_at(&self.abar, t[i / dim]).min(1.0 - 1e-6);
            out[i] = x0_to_velocity(a, x[i], out[i]);
        }
    }
}

/// 单隐层 tanh MLP 速度场：`v = W₂·tanh(W₁·[x; cond; t] + b₁) + b₂`。
///
/// 用途有两件：给「图 → 3D」演示当条件场，以及给批处理读数一个**真批友好**的场
/// （行主序矩阵乘，批越大越省每样本开销）。它不负责训练；权重由外部优化器写进字段。
#[derive(Debug, Clone, PartialEq)]
pub struct MlpField {
    /// 状态维数。
    pub dim: usize,
    /// 条件维数。
    pub cond_dim: usize,
    /// 隐层宽度。
    pub hid: usize,
    /// `[hid][dim + cond_dim + 1]` 行主序。
    pub w1: Vec<f32>,
    /// `[hid]`。
    pub b1: Vec<f32>,
    /// `[dim][hid]` 行主序。
    pub w2: Vec<f32>,
    /// `[dim]`。
    pub b2: Vec<f32>,
}

impl MlpField {
    /// 用固定种子做小权重初始化（确定性：同种子同权重）。
    ///
    /// # Panics
    /// 任一维数为 0。
    #[must_use]
    pub fn new(dim: usize, cond_dim: usize, hid: usize, seed: u64) -> Self {
        assert!(dim > 0 && hid > 0, "维数必须为正");
        let nin = dim + cond_dim + 1;
        let mut rng = SeedRng::new(seed);
        let mut uniform = |n: usize, scale: f32| -> Vec<f32> {
            (0..n)
                .map(|_| {
                    let u = (rng.next_u64() >> 40) as f32 / (1u32 << 24) as f32; // [0,1)
                    (u * 2.0 - 1.0) * scale
                })
                .collect()
        };
        let s1 = (1.0 / nin as f32).sqrt();
        let s2 = (1.0 / hid as f32).sqrt();
        Self {
            dim,
            cond_dim,
            hid,
            w1: uniform(hid * nin, s1),
            b1: vec![0.0; hid],
            w2: uniform(dim * hid, s2),
            b2: vec![0.0; dim],
        }
    }

    /// 输入拼接口径：`[x; cond; t]`，长度 `dim + cond_dim + 1`。
    #[must_use]
    pub fn nin(&self) -> usize {
        self.dim + self.cond_dim + 1
    }

    /// 单个样本的前向（`z` 已按 [`MlpField::nin`] 拼好），写进 `out`。
    pub fn forward_one(&self, z: &[f32], out: &mut [f32]) {
        let mut h = vec![0.0f32; self.hid];
        for (j, hj) in h.iter_mut().enumerate() {
            let row = &self.w1[j * self.nin()..(j + 1) * self.nin()];
            let mut acc = self.b1[j];
            for (w, zz) in row.iter().zip(z) {
                acc += w * zz;
            }
            *hj = acc.tanh();
        }
        for (i, o) in out.iter_mut().take(self.dim).enumerate() {
            let row = &self.w2[i * self.hid..(i + 1) * self.hid];
            let mut acc = self.b2[i];
            for (w, hh) in row.iter().zip(&h) {
                acc += w * hh;
            }
            *o = acc;
        }
    }
}

impl Field for MlpField {
    fn dim(&self) -> usize {
        self.dim
    }
    fn cond_dim(&self) -> usize {
        self.cond_dim
    }
    fn eval(&self, x: &[f32], t: &[f32], cond: &[f32], out: &mut [f32]) {
        let (dim, cd, nin, hid) = (self.dim, self.cond_dim, self.nin(), self.hid);
        let n = t.len();
        debug_assert_eq!(x.len(), n * dim);
        debug_assert_eq!(cond.len(), n * cd);
        let mut z = vec![0.0f32; nin];
        let mut h = vec![0.0f32; hid];
        for b in 0..n {
            z[..dim].copy_from_slice(&x[b * dim..(b + 1) * dim]);
            z[dim..dim + cd].copy_from_slice(&cond[b * cd..(b + 1) * cd]);
            z[dim + cd] = t[b];
            for (j, hj) in h.iter_mut().enumerate() {
                let row = &self.w1[j * nin..(j + 1) * nin];
                let mut acc = self.b1[j];
                for (w, zz) in row.iter().zip(&z) {
                    acc += w * zz;
                }
                *hj = acc.tanh();
            }
            for i in 0..dim {
                let row = &self.w2[i * hid..(i + 1) * hid];
                let mut acc = self.b2[i];
                for (w, hh) in row.iter().zip(&h) {
                    acc += w * hh;
                }
                out[b * dim + i] = acc;
            }
        }
    }
}

//! Burgers 治理示范：数值实验与普通命令一样有准入 / 不变量 / 作废语义。
//!
//! # 放在 tests/ 而不是 examples/ 的理由
//!
//! tests 进 `cargo test -p trit-ops`（本仓收工门的组成部分），这个示范的
//! 每一次回归都会被闸**真正执行**——"实验也过闸"的前提是实验在闸的射程内。
//! examples/ 只在有人手动 `cargo run --example` 时才跑、不在任何门里：
//! 治理示范放在不被闸跑的位置上，等于示范自我否定。
//!
//! # 这是什么、不是什么
//!
//! 这是**治理结构的演示**：数值实验启动前过准入帽（CFL 数超阈值拒绝启动
//! ——`run_caps`/policy"资源帽先于执行、越帽拒绝"在数值世界的同构）；
//! 运行中每步**再过准入**（dt 逐步供给，同"每条命令都过闸"，且 umax 实测
//! 自当前状态、不信任最大值原理）并核对**能量漂移不变量**（每步能量与
//! 解析衰减界比对，越界 panic = 实验作废，证据不可再用——trace invariants
//! 的 fail-closed 同构）；跑通的实验必须回报带证据的 [`RunRecord`]（同
//! 判定层"结论必须带证据"）。
//!
//! 它**不是** NS 定理的任何声明，也不声称证明任何流体力学命题——对 NS
//! 主题的诚实致敬方式是治理数值实验，不是声称定理。1D 粘性 Burgers
//! `u_t + u·u_x = ν·u_xx` 是标准教学方程；迎风格式的 CFL 稳定性条件与
//! 周期域能量衰减见任何数值 PDE 教材（如 LeVeque, *Finite Volume Methods
//! for Hyperbolic Problems*, 2002）；准人用的 CFL 数按
//! `trit_ops::interval` 的保守区间包络核对（点区间进、加宽 1 ulp 出，
//! 边界处宁可拒）。
//!
//! # 治理面与 trit-orch 的同构
//!
//! | 编排世界 | 数值世界（本文件） |
//! |---|---|
//! | run_caps/policy：资源帽先于执行，越帽拒绝 | [`Experiment::plan`]：CFL 帽先于演化，超帽拒绝启动 |
//! | 每条命令过闸（policy 分类先于执行） | 每步 [`Experiment::step`] 先核 CFL（dt 逐步供给） |
//! | trace invariants 破红 fail-closed | 能量漂移不变量越界 panic = 实验作废 |
//! | 结论必须带证据（回执） | [`RunRecord`] 带 cfl/e0/e_final/界，断言可对账 |

use std::fmt;

use trit_ops::interval::Interval;

/// 扩散 CFL 帽：显式格式解析稳定界是 0.5（FTCS 热传导项），治理帽收紧
/// 10%。准入帽不是稳定边界本身——与资源帽"内核帽不是安全边界"同一取向：
/// 帽在解析界之内留余量。
const DIFFUSIVE_CFL_CAP: f64 = 0.45;
/// 对流 CFL 帽：迎风格式解析稳定界是 1.0，治理帽收紧 10%。
const CONVECTIVE_CFL_CAP: f64 = 0.9;
/// 能量不变量松弛：连续 Poincaré 衰减命题**不逐步传递**到离散解（显式格式
/// 没有对应的离散不等式可引用），核对带 1% 治理余量。如实说明：本口径实测
/// 零松弛也绿（迎风耗散主导，实测于 `compliant_run_passes…`，改常量即可
/// 复验）；1% 是参数变化下的余量，不声称实测必要性。
const ENERGY_SLACK: f64 = 0.01;

/// 准入拒绝原因（机器可读；测试按形态匹配，不解析文案）。
#[derive(Debug, Clone, Copy, PartialEq)]
enum Rejection {
    /// 扩散 CFL 数（ν·Δt/Δx² 的区间包络上端）超帽。
    DiffusiveCfl { value: f64, cap: f64 },
    /// 对流 CFL 数（|u|max·Δt/Δx 的区间包络上端）超帽。
    ConvectiveCfl { value: f64, cap: f64 },
}

impl fmt::Display for Rejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Rejection::DiffusiveCfl { value, cap } => {
                write!(f, "扩散 CFL 数 {value:.4} 超帽 {cap}")
            }
            Rejection::ConvectiveCfl { value, cap } => {
                write!(f, "对流 CFL 数 {value:.4} 超帽 {cap}")
            }
        }
    }
}

/// CFL 数按点区间走 `interval` 四则——入口点区间、出口加宽 1 ulp。
/// 核对用包络**上端**：恰在帽上的值经加宽后必然越帽，治理在边界处宁可拒。
/// 扩散 CFL 数的保守包络：ν·Δt/Δx²。
fn cfl_diffusive(nu: f64, dt: f64, dx: f64) -> Interval {
    Interval::point(nu)
        .mul(Interval::point(dt))
        .div(Interval::point(dx).mul(Interval::point(dx)))
}

/// 对流 CFL 数的保守包络：|u|max·Δt/Δx。
fn cfl_convective(umax: f64, dt: f64, dx: f64) -> Interval {
    Interval::point(umax)
        .mul(Interval::point(dt))
        .div(Interval::point(dx))
}

/// 准入检查：CFL 包络上端 ≤ 帽。包络为 Empty（无法给出保守答案）也拒
/// ——fail-closed 与 run_caps 同构：没有保守判据就不启动。
fn check_cfl(nu: f64, dt: f64, dx: f64, umax: f64) -> Result<(), Rejection> {
    let d = cfl_diffusive(nu, dt, dx);
    if d.is_empty() || d.hi() > DIFFUSIVE_CFL_CAP {
        return Err(Rejection::DiffusiveCfl {
            value: if d.is_empty() { f64::NAN } else { d.hi() },
            cap: DIFFUSIVE_CFL_CAP,
        });
    }
    let c = cfl_convective(umax, dt, dx);
    if c.is_empty() || c.hi() > CONVECTIVE_CFL_CAP {
        return Err(Rejection::ConvectiveCfl {
            value: if c.is_empty() { f64::NAN } else { c.hi() },
            cap: CONVECTIVE_CFL_CAP,
        });
    }
    Ok(())
}

/// 能量 E = ½∫u²dx（周期域离散和）。
fn energy(u: &[f64], dx: f64) -> f64 {
    0.5 * dx * u.iter().map(|&ui| ui * ui).sum::<f64>()
}

/// 解析衰减界：周期域上 ∫u_x² ≥ k₁²∫u²（Poincaré，k₁ = 2π/L），而
/// E'(t) = −2ν∫u_x²dx ≤ −2νk₁²E ⟹ E(t) ≤ E(0)·e^(−2νk₁²t)。核对带
/// [`ENERGY_SLACK`] 离散化余量（见常量注释）。
fn decay_bound(e0: f64, nu: f64, t: f64) -> f64 {
    let k1 = 2.0 * std::f64::consts::PI; // 域长 L = 1
    e0 * (-(2.0 * nu * k1 * k1) * t).exp() * (1.0 + ENERGY_SLACK)
}

/// 跑通的实验回报的记录（"结论带证据"：数值全部可对账，不是一句"跑完了"）。
#[derive(Debug, Clone, Copy)]
struct RunRecord {
    steps: usize,
    /// 终点时刻。
    t: f64,
    /// 初始能量。
    e0: f64,
    /// 终点能量（≤ bound_final，逐步断言的终态）。
    e_final: f64,
    /// 终点解析衰减界（含离散化余量）。
    bound_final: f64,
    /// 启动时的 CFL 包络上端（证据）。
    cfl_diffusive_hi: f64,
    cfl_convective_hi: f64,
}

/// 1D 粘性 Burgers 显式解算器：迎风对流 + 中心差分扩散，周期域 [0,1)。
/// 演化只在准入与不变量的闸内发生。
#[derive(Debug)]
struct Experiment {
    u: Vec<f64>,
    nu: f64,
    dx: f64,
    /// 名义步长（`run` 用它逐步步进；注入测试可直接调 [`Experiment::step`]）。
    dt: f64,
    /// 计划步数（run 的循环只受它约束）。
    plan_steps: usize,
    steps: usize,
    t: f64,
    e0: f64,
}

impl Experiment {
    /// 启动 = 先过准入（超帽拒绝启动——不分配不演化，同 run_caps 先于执行）。
    /// 参数体检也在这里：非有限/非正的 ν·dt·dx 不给"带病启动"的机会。
    fn plan(u0: Vec<f64>, nu: f64, dx: f64, dt: f64, steps: usize) -> Result<Self, Rejection> {
        if u0.is_empty() || !u0.iter().all(|v| v.is_finite()) {
            panic!("初始剖面必须非空且有限");
        }
        if !(nu.is_finite() && nu >= 0.0 && dx.is_finite() && dx > 0.0) {
            panic!("ν 必须非负有限、Δx 必须正有限");
        }
        let umax = umax_of(&u0);
        check_cfl(nu, dt, dx, umax)?;
        let e0 = energy(&u0, dx);
        Ok(Self {
            u: u0,
            nu,
            dx,
            dt,
            plan_steps: steps,
            steps: 0,
            t: 0.0,
            e0,
        })
    }

    /// 单步 = 逐步准入（dt 逐步供给）→ 演化 → 有限性检查 → 能量不变量。
    /// 任何一环判红即 panic = 实验作废（证据不可再用）。
    fn step(&mut self, dt: f64) {
        // 逐步再准入：不信任"启动时查过"——状态会演化、dt 可能被改。
        // umax 实测自当前状态，不信任最大值原理（治理测量，不假设）。
        let umax = umax_of(&self.u);
        if let Err(r) = check_cfl(self.nu, dt, self.dx, umax) {
            panic!("burgers 准入拒绝（第 {} 步）：{r}——实验作废", self.steps);
        }
        // 迎风对流（按 u 的符号选差分方向）+ 中心差分扩散。
        let n = self.u.len();
        let mut next = Vec::with_capacity(n);
        for i in 0..n {
            let im = (i + n - 1) % n;
            let ip = (i + 1) % n;
            let ui = self.u[i];
            let grad = if ui >= 0.0 {
                (ui - self.u[im]) / self.dx
            } else {
                (self.u[ip] - ui) / self.dx
            };
            let lap = (self.u[ip] - 2.0 * ui + self.u[im]) / (self.dx * self.dx);
            next.push(ui - dt * ui * grad + dt * self.nu * lap);
        }
        self.u = next;
        self.t += dt;
        self.steps += 1;
        // 有限性检查（fail fast，不让 NaN 走进能量账本）。
        for (i, v) in self.u.iter().enumerate() {
            assert!(
                v.is_finite(),
                "burgers 状态非有限（第 {} 步，格点 {i}）——实验作废",
                self.steps
            );
        }
        // 能量漂移不变量：每步与解析衰减界比对，越界即作废。
        let e = energy(&self.u, self.dx);
        let bound = decay_bound(self.e0, self.nu, self.t);
        assert!(
            e <= bound,
            "burgers 能量不变量破红（第 {} 步）：E={e:.6e} > 解析衰减界 {bound:.6e}——实验作废",
            self.steps
        );
    }

    /// 按 [`Experiment::dt`] 跑完计划步数，回报带证据的记录。
    fn run(mut self) -> RunRecord {
        let dt = self.dt;
        while self.steps < self.plan_steps {
            self.step(dt);
        }
        RunRecord {
            steps: self.steps,
            t: self.t,
            e0: self.e0,
            e_final: energy(&self.u, self.dx),
            bound_final: decay_bound(self.e0, self.nu, self.t),
            cfl_diffusive_hi: cfl_diffusive(self.nu, dt, self.dx).hi(),
            cfl_convective_hi: cfl_convective(umax_of(&self.u), dt, self.dx).hi(),
        }
    }
}

/// 当前状态实测的最大 |u|（治理测量，不假设）。
fn umax_of(u: &[f64]) -> f64 {
    u.iter().fold(0.0_f64, |m, &v| m.max(v.abs()))
}

/// 平滑初始剖面：双正弦叠加（有粘性衰减也有非线性陡化可观察）。
fn smooth_profile(n: usize) -> Vec<f64> {
    (0..n)
        .map(|i| {
            let x = i as f64 / n as f64;
            0.5 * (2.0 * std::f64::consts::PI * x).sin()
                + 0.2 * (4.0 * std::f64::consts::PI * x).sin()
        })
        .collect()
}

#[test]
fn cfl_violation_rejected_before_start() {
    // 对流违规：ν = 0.001（扩散 CFL ≈ 0.328 ≤ 0.45 不抢跑），
    // umax ≈ 0.60，dt = 0.02，Δx = 1/128 → 0.60·0.02·128 ≈ 1.55 > 0.9。
    let r = Experiment::plan(smooth_profile(128), 0.001, 1.0 / 128.0, 0.02, 10);
    assert!(
        matches!(r, Err(Rejection::ConvectiveCfl { .. })),
        "对流超帽必须拒绝启动：{r:?}"
    );
    // 扩散违规：ν = 0.1 → 0.1·0.002·16384 ≈ 3.28 > 0.45。
    let r = Experiment::plan(smooth_profile(128), 0.1, 1.0 / 128.0, 0.002, 10);
    assert!(
        matches!(r, Err(Rejection::DiffusiveCfl { .. })),
        "扩散超帽必须拒绝启动：{r:?}"
    );
    // 两种帽都在边界附近有行为（0.4510 拒 / 0.4490 过——余量 ≫ 加宽幅度，
    // 断言稳定；"恰在帽上必拒"由包络上端语义保证，见 cfl_envelope 注释）。
    let dx = 1.0 / 128.0;
    let dt = 0.002;
    let nu_over = 0.4510 / (dt * 128.0 * 128.0);
    assert!(matches!(
        Experiment::plan(smooth_profile(128), nu_over, dx, dt, 10),
        Err(Rejection::DiffusiveCfl { .. })
    ));
    let nu_under = 0.4490 / (dt * 128.0 * 128.0);
    assert!(
        Experiment::plan(smooth_profile(128), nu_under, dx, dt, 10).is_ok(),
        "帽下 0.4490 必须放行"
    );
}

#[test]
fn compliant_run_passes_and_energy_stays_under_analytic_bound() {
    // ν = 0.01，Δx = 1/128，dt = 0.002：扩散 CFL ≈ 0.328 ≤ 0.45，
    // 对流 CFL ≈ 0.155 ≤ 0.9；1000 步 → t = 2，解析衰减因子 e^(−2ν(2π)²t)
    // = e^(−1.579) ≈ 0.206——不变量要求每步能量都在界内。
    let exp = Experiment::plan(smooth_profile(128), 0.01, 1.0 / 128.0, 0.002, 1000)
        .expect("合规配置必须过准入");
    let rec = exp.run();
    assert_eq!(rec.steps, 1000);
    assert!((rec.t - 2.0).abs() < 1e-9, "终点时刻必须对账：{}", rec.t);
    assert!(
        rec.cfl_diffusive_hi <= DIFFUSIVE_CFL_CAP && rec.cfl_convective_hi <= CONVECTIVE_CFL_CAP,
        "记录里的 CFL 证据必须与帽一致：{rec:?}"
    );
    assert!(
        rec.e_final <= rec.bound_final,
        "终点能量必须保持在解析衰减界内：{} > {}",
        rec.e_final,
        rec.bound_final
    );
    assert!(
        rec.e_final < 0.5 * rec.e0,
        "粘性衰减必须真实发生（物理非空转）：{} ≥ {}",
        rec.e_final,
        rec.e0
    );
}

#[test]
#[should_panic(expected = "burgers 准入拒绝")]
fn injected_violating_dt_flags_red_midrun() {
    // 合规启动跑 10 步，第 11 步注入违规 dt（0.02 → 对流 CFL ≈ 1.55）：
    // 逐步准入当场判红——实验作废，不等到终点才被发现。
    let mut exp = Experiment::plan(smooth_profile(128), 0.01, 1.0 / 128.0, 0.002, 100)
        .expect("合规配置必须过准入");
    for _ in 0..10 {
        exp.step(0.002);
    }
    exp.step(0.02);
}

#[test]
#[should_panic(expected = "burgers 能量不变量破红")]
fn corrupted_state_flags_energy_invariant_red() {
    // CFL 仍然合规的污染：状态 ×1.5（对流 CFL 0.155·1.5 ≈ 0.23 ✓）但能量
    // 瞬间 ×2.25 ≫ 1% 余量——数据被改而稳定域没被破坏，能量账本必须看得
    // 出来（trace invariants"证据被替换必须可检出"的数值同构）。
    let mut exp = Experiment::plan(smooth_profile(128), 0.01, 1.0 / 128.0, 0.002, 100)
        .expect("合规配置必须过准入");
    exp.step(0.002);
    for v in exp.u.iter_mut() {
        *v *= 1.5;
    }
    exp.step(0.002);
}

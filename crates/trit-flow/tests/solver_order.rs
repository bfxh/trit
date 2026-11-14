//! 积分阶数对拍：对角线性漂移场有闭式解，误差随步长按方案阶数下降。
//!
//! Euler 一阶（步数加倍误差约减半），Midpoint / Heun 二阶（约减到四分之一）。
//! 这条红的含义：`advance` 的三套更新式与数学定义不符（例如 Heun 少乘 0.5、Δt 符号错）。

use trit_flow::{LinearDrift, Scheme, Scratch, Solver, advance};

const A: f32 = -1.0;
const B: f32 = 0.5;
const X0: f32 = 0.2;

/// `dx/dt = a(x − b)` 从 `t = 1` 到 `t = 0` 的闭式解。
fn exact() -> f32 {
    B + (X0 - B) * (-A).exp()
}

fn integrate(scheme: Scheme, steps: usize) -> f32 {
    let field = LinearDrift {
        a: vec![A; 4],
        b: vec![B; 4],
    };
    let mut x = vec![X0; 4];
    let mut scratch = Scratch::new();
    let solver = Solver::new(scheme, steps);
    let times = solver.times();
    for &tk in &times[..steps] {
        advance(
            &field,
            scheme,
            &mut x,
            &[tk],
            &[solver.dt()],
            &[],
            &mut scratch,
        );
    }
    // 四维同步，取任一维即可
    x[0]
}

fn err(scheme: Scheme, steps: usize) -> f64 {
    (integrate(scheme, steps) as f64 - exact() as f64).abs()
}

#[test]
fn euler_is_first_order() {
    let (e8, e16) = (err(Scheme::Euler, 8), err(Scheme::Euler, 16));
    let (e32, e64) = (err(Scheme::Euler, 32), err(Scheme::Euler, 64));
    assert!(e16 < e8, "步数加倍误差应下降：{e8:.3e} → {e16:.3e}");
    assert!(e32 < e16 && e64 < e32);
    // 一阶：相邻档误差比在 1.7–2.3 之间
    for (lo, hi) in [(e8, e16), (e16, e32), (e32, e64)] {
        let ratio = lo / hi;
        assert!(
            (1.7..=2.3).contains(&ratio),
            "Euler 一阶收敛比应在 [1.7,2.3]，实测 {ratio:.3}（{lo:.3e} → {hi:.3e}）"
        );
    }
}

#[test]
fn midpoint_is_second_order() {
    let (e8, e16) = (err(Scheme::Midpoint, 8), err(Scheme::Midpoint, 16));
    let (e16b, e32) = (e16, err(Scheme::Midpoint, 32));
    for (lo, hi) in [(e8, e16), (e16b, e32)] {
        let ratio = lo / hi;
        assert!(
            (3.4..=4.6).contains(&ratio),
            "Midpoint 二阶收敛比应在 [3.4,4.6]，实测 {ratio:.3}"
        );
    }
}

#[test]
fn heun_is_second_order() {
    let (e8, e16) = (err(Scheme::Heun, 8), err(Scheme::Heun, 16));
    let (e16b, e32) = (e16, err(Scheme::Heun, 32));
    for (lo, hi) in [(e8, e16), (e16b, e32)] {
        let ratio = lo / hi;
        assert!(
            (3.4..=4.6).contains(&ratio),
            "Heun 二阶收敛比应在 [3.4,4.6]，实测 {ratio:.3}"
        );
    }
}

#[test]
fn constant_field_is_exact_for_euler() {
    // v = 常数 ⇒ Euler 一步精确（误差只来自浮点）
    let field = trit_flow::Constant { v: vec![2.0; 3] };
    let mut x = vec![0.0; 3];
    let mut scratch = Scratch::new();
    advance(
        &field,
        Scheme::Euler,
        &mut x,
        &[1.0],
        &[-0.5],
        &[],
        &mut scratch,
    );
    for v in x {
        assert!((v + 1.0).abs() < 1e-6, "2.0 × (−0.5) = −1.0，实测 {v}");
    }
}

#[test]
fn times_grid_is_monotone() {
    let s = Solver::new(Scheme::Euler, 10);
    let t = s.times();
    assert_eq!(t.len(), 11);
    assert!((t[0] - 1.0).abs() < 1e-7);
    assert!(t[10].abs() < 1e-7);
    for w in t.windows(2) {
        assert!(w[1] < w[0], "时间网格必须严格递减");
    }
}

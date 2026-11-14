//! 图 → 3D 漂移场：端到端管线 + 并发读数。
//!
//! 三个场，同一套 `Field` 接口、同一个执行器：
//! 1. **理想漂移场**（解析，`v = k·(x₁(c) − x)`）：判端到端正确性——积分终点必须回到
//!    由图像条件决定的 8³ 占据场，误差可算；
//! 2. **MLP 漂移场**（512→64→512，未训练）：给真实计算量下的延迟/吞吐读数；
//! 3. **扩散侧**（`x̂₀` 预测器 + 余弦 ᾱ 表，经 [`X0ToVelocity`] 桥）：证明扩散与漂移
//!    走的是**同一条**执行器路径。
//!
//! 诚实边界：MLP 权重是随机初始化的，读数只证明「并发层 + 管线」，不证明生成质量。
//!
//! 跑法：`cargo run --release -p trit-flow --example img23d_flow`

use std::sync::Arc;

use trit_bench::SeedRng;
use trit_flow::{Cfg, Executor, Field, MlpField, Request, Scheme, X0Predictor, X0ToVelocity};

/// 体素边长。
const VOX: usize = 8;
/// 状态维数 = 体素数。
const DIM: usize = VOX * VOX * VOX;
/// 图像条件维数（真实系统里是从图像编码器取的特征）。
const COND: usize = 16;

/// 图像条件 → 目标占据场：按 `cond[0..3]` 的 argmax 选球 / 立方 / 柱。
fn target(cond: &[f32]) -> Vec<f32> {
    let which = if cond[0] >= cond[1] && cond[0] >= cond[2] {
        0
    } else if cond[1] >= cond[2] {
        1
    } else {
        2
    };
    let c = (VOX as f32 - 1.0) / 2.0;
    let mut out = vec![0.0f32; DIM];
    for z in 0..VOX {
        for y in 0..VOX {
            for x in 0..VOX {
                let (fx, fy, fz) = (x as f32 - c, y as f32 - c, z as f32 - c);
                let inside = match which {
                    0 => (fx * fx + fy * fy + fz * fz).sqrt() <= 2.2,
                    1 => fx.abs() <= 2.0 && fy.abs() <= 2.0 && fz.abs() <= 2.0,
                    _ => (fx * fx + fy * fy).sqrt() <= 2.2 && fz.abs() <= 2.6,
                };
                out[x + VOX * (y + VOX * z)] = f32::from(u8::from(inside));
            }
        }
    }
    out
}

/// 理想漂移场：`v = k·(x − x₁(c))`。
///
/// 约定 `t: 1 噪声 → 0 数据`、`Δ t < 0`，所以朝向数据的收敛速度必须写成 `k(x − x₁)`
/// （闭式解 `x(t) = x₁ + (x(1) − x₁)e^{k(t−1)}`）；写成 `k(x₁ − x)` 是指数**发散**。
/// `k=16`、64 步时 Euler 的衰减因子是 `1 − k/64 = 0.75`，残留 `0.75^64 ≈ 1e-8`。
struct IdealDrift {
    k: f32,
}

impl Field for IdealDrift {
    fn dim(&self) -> usize {
        DIM
    }
    fn cond_dim(&self) -> usize {
        COND
    }
    fn eval(&self, x: &[f32], _t: &[f32], cond: &[f32], out: &mut [f32]) {
        for b in 0..cond.len() / COND {
            let tgt = target(&cond[b * COND..(b + 1) * COND]);
            for i in 0..DIM {
                out[b * DIM + i] = self.k * (x[b * DIM + i] - tgt[i]);
            }
        }
    }
}

/// 理想去噪器：直接输出目标占据场（扩散侧的解析上限）。
struct IdealX0;

impl X0Predictor for IdealX0 {
    fn dim(&self) -> usize {
        DIM
    }
    fn cond_dim(&self) -> usize {
        COND
    }
    fn predict_x0(&self, _x: &[f32], _t: &[f32], cond: &[f32], out: &mut [f32]) {
        for b in 0..cond.len() / COND {
            let tgt = target(&cond[b * COND..(b + 1) * COND]);
            out[b * DIM..(b + 1) * DIM].copy_from_slice(&tgt);
        }
    }
}

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

fn noise(seed: u64, n: usize) -> Vec<f32> {
    let mut rng = SeedRng::new(seed);
    (0..n)
        .map(|_| (rng.next_u64() >> 40) as f32 / (1u32 << 24) as f32)
        .collect()
}

fn conds(n: usize) -> Vec<Vec<f32>> {
    (0..n).map(|i| noise(1000 + i as u64, COND)).collect()
}

fn requests(cs: &[Vec<f32>], steps: usize) -> Vec<Request> {
    cs.iter()
        .enumerate()
        .map(|(i, c)| Request::with_cond(noise(i as u64 + 1, DIM), c.clone(), steps, Scheme::Euler))
        .collect()
}

fn l2(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y) * (x - y))
        .sum::<f32>()
        .sqrt()
}

/// 打印一行读数。
fn row(name: &str, r: &trit_flow::Report) {
    println!(
        "{:<22} {:>8.2} {:>8.2} {:>8.2} {:>9.1} {:>12.0} {:>7.1}",
        name,
        r.latency.p50,
        r.latency.p90,
        r.latency.p99,
        r.rps,
        r.sample_steps_per_s,
        r.mean_batch
    );
}

fn main() {
    let steps = 32;
    let n_req = 64;
    let cs = conds(n_req);

    println!("== 图 → 3D 漂移场（8³ 体素 = {DIM} 维，条件 {COND} 维）==");
    println!();

    // 1) 正确性：理想漂移场，64 步
    {
        let ex = Executor::new(
            Arc::new(IdealDrift { k: 16.0 }),
            Cfg {
                threads: 4,
                max_batch: 16,
            },
        );
        let rs = requests(&cs, 64);
        let (out, rep) = ex.run(&rs).expect("理想漂移场");
        let mut worst = 0.0f32;
        for (i, o) in out.iter().enumerate() {
            let tgt = target(&cs[i]);
            worst = worst.max(l2(&o.x, &tgt));
        }
        println!("正确性（理想漂移场 v = 16·(x − x₁)，64 步 Euler）：");
        println!(
            "  最大 L2 误差 = {worst:.3e}（期望 < 1e-3），墙钟 {:.1} ms，{:.0} 样本·步/秒",
            rep.wall_ms, rep.sample_steps_per_s
        );
        assert!(worst < 1e-3, "漂移场终点应回到目标形状，实测 L2 = {worst}");
        println!();
    }

    // 2) 读数：MLP 漂移场（真实计算量）
    let field: Arc<dyn Field + Send + Sync> = Arc::new(MlpField::new(DIM, COND, 64, 7));
    let rs = requests(&cs, steps);
    println!("读数（MLP 漂移场 512→64→512，{n_req} 请求 × {steps} 步 Euler；权重未训练）",);
    println!(
        "{:<22} {:>8} {:>8} {:>8} {:>9} {:>12} {:>7}",
        "配置", "p50/ms", "p90/ms", "p99/ms", "req/s", "样本·步/秒", "平均批"
    );
    let cfgs = [
        (
            "串行 batch=1",
            Cfg {
                threads: 0,
                max_batch: 1,
            },
        ),
        (
            "批 batch=32",
            Cfg {
                threads: 0,
                max_batch: 32,
            },
        ),
        (
            "批+并行 8 线程",
            Cfg {
                threads: 8,
                max_batch: 32,
            },
        ),
        (
            "批+并行 16 线程",
            Cfg {
                threads: 16,
                max_batch: 64,
            },
        ),
    ];
    for (name, cfg) in cfgs {
        let ex = Executor::new(Arc::clone(&field), cfg);
        let (_, rep) = ex.run(&rs).expect("MLP 场");
        row(name, &rep);
    }
    println!();

    // 3) 扩散侧：同一个执行器，换一个场（x̂₀ 预测 + ᾱ 桥）
    println!("扩散侧（x̂₀ 预测器 + ᾱ 桥，同一执行器；Euler 一阶，看步数收敛）");
    println!(
        "{:<22} {:>8} {:>8} {:>8} {:>9} {:>12} {:>7} {:>9}",
        "配置", "p50/ms", "p90/ms", "p99/ms", "req/s", "样本·步/秒", "平均批", "L2 误差"
    );
    for steps_d in [32usize, 128, 512] {
        let abar = abar_table(steps_d);
        let diff: Arc<dyn Field + Send + Sync> = Arc::new(X0ToVelocity::new(IdealX0, abar));
        let rs_d = requests(&cs, steps_d);
        for (name, cfg) in [
            (
                "串行 batch=1",
                Cfg {
                    threads: 0,
                    max_batch: 1,
                },
            ),
            (
                "批+并行 8 线程",
                Cfg {
                    threads: 8,
                    max_batch: 32,
                },
            ),
        ] {
            let ex = Executor::new(Arc::clone(&diff), cfg);
            let (out, rep) = ex.run(&rs_d).expect("扩散场");
            let worst = out
                .iter()
                .enumerate()
                .map(|(i, o)| l2(&o.x, &target(&cs[i])))
                .fold(0.0f32, f32::max);
            println!(
                "{:<22} {:>8.2} {:>8.2} {:>8.2} {:>9.1} {:>12.0} {:>7.1} {:>9.3e}",
                format!("{} / {} 步", name, steps_d),
                rep.latency.p50,
                rep.latency.p90,
                rep.latency.p99,
                rep.rps,
                rep.sample_steps_per_s,
                rep.mean_batch,
                worst
            );
        }
    }
    println!();
    println!("注：延迟含每轮组批与片间拷贝；`样本·步/秒` 是跨配置可比的吞吐口径。");
    println!("注：`批 batch=32` 相对串行几乎无收益——本仓的 MLP 场是逐样本循环，批不改变算术；");
    println!("    批的收益在这里来自摊薄每轮执行器开销，真正的批收益要等场把一次 eval 映射成");
    println!("    一次 GPU/矩阵乘启动（那时批越大越省）。并行收益则是直接的。");
}

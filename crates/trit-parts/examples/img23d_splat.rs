//! 占位真实场：**图 + 深度 → 3DGS splat**，走 `trit-flow` 的统一 `Field` / `Executor`。
//!
//! 管线（每一段都是仓里现成的件）：
//! 1. 程序生成 3 个形状的 splat 模板（球 / 立方壳 / 柱），每个 [`N`] 个高斯；
//! 2. 用 `orbit_camera` + `collect` / `render_from` 渲染「图」与「深度」（占位输入：
//!    真实系统里这两张来自相机与深度传感器，接口相同）；
//! 3. 从图 + 深度提条件向量（8×8 网格的深度均值 + 亮度 = 128 维）；
//! 4. [`SplatField`]：`dim = N×8`（pos3 + scale + color3 + alpha），做**检索式占位场**
//!    `v = k·(x − x₁(cond))`——不训练，但接口与将来学出来的场完全一致；
//! 5. `trit-flow` 的 `Executor` 跑 32 步 Euler；
//! 6. 解码成 `Vec<Splat>` → 写 **标准 3DGS PLY** → 读回逐项对账。
//!
//! 诚实边界：这是**占位**，不是模型。权重（模板）来自程序生成，`v` 是解析的；
//! 换真权重时只换 [`SplatField::eval`] 的内部，接口与执行器不动。
//!
//! 跑法：`cargo run --release -p trit-parts --example img23d_splat`

use std::sync::Arc;

use trit_flow::{Cfg, Executor, Field, Request, Scheme};
use trit_parts::geo::Camera;
use trit_parts::splat::{Splat, collect, orbit_camera, read_ply_3dgs, render_from, write_ply_3dgs};

/// 每个形状的高斯数。
const N: usize = 256;
/// 每高斯参数量：pos(3) + scale(1) + color(3) + alpha(1)。
const PER: usize = 8;
/// 场维数。
const DIM: usize = N * PER;
/// 渲染分辨率。
const RES: usize = 64;
/// 条件网格边长（8×8）。
const G: usize = 8;
/// 条件维数：深度均值 + 亮度。
const COND: usize = G * G * 2;
/// 积分步数。
const STEPS: usize = 32;

/// 确定性伪随机（不用外部 RNG：形状生成必须可复现）。
fn det(i: usize) -> f32 {
    let x = (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let x = x ^ (x >> 29);
    ((x >> 40) as f32 / (1u32 << 24) as f32) * 2.0 - 1.0
}

fn splat(pos: [f32; 3]) -> Splat {
    // 颜色按位置给（便于肉眼看形状），alpha 固定
    Splat {
        pos,
        scale: 0.07,
        color: [0.5 + 0.4 * pos[0], 0.5 + 0.4 * pos[1], 0.5 + 0.4 * pos[2]],
        alpha: 0.9,
    }
}

/// 球面（Fibonacci 采样，各向同性）。
fn sphere() -> Vec<Splat> {
    let mut out = Vec::with_capacity(N);
    let ga = std::f32::consts::PI * (3.0 - 5f32.sqrt());
    for i in 0..N {
        let y = 1.0 - 2.0 * (i as f32 + 0.5) / N as f32;
        let r = (1.0 - y * y).max(0.0).sqrt();
        let th = ga * i as f32;
        out.push(splat([0.5 * r * th.cos(), 0.5 * y, 0.5 * r * th.sin()]));
    }
    out
}

/// 立方壳（6 面均匀取点）。
fn cube() -> Vec<Splat> {
    let mut out = Vec::with_capacity(N);
    let side = 6;
    let per_face = N.div_ceil(6);
    for i in 0..N {
        let face = i / per_face;
        let k = i % per_face;
        let (u, v) = (
            -0.5 + (k % side) as f32 / (side - 1) as f32,
            -0.5 + (k / side) as f32 / (side - 1).max(1) as f32,
        );
        let p = match face {
            0 => [u, v, 0.5],
            1 => [u, v, -0.5],
            2 => [u, 0.5, v],
            3 => [u, -0.5, v],
            4 => [0.5, u, v],
            _ => [-0.5, u, v],
        };
        out.push(splat(p));
    }
    out
}

/// 柱（侧面 + 上下底）。
fn cylinder() -> Vec<Splat> {
    let mut out = Vec::with_capacity(N);
    let side = 160;
    for i in 0..side {
        let th = std::f32::consts::TAU * i as f32 / side as f32;
        let h = -0.5 + (i % 9) as f32 / 8.0;
        out.push(splat([0.35 * th.cos(), h, 0.35 * th.sin()]));
    }
    let rest = N - side;
    for i in 0..rest {
        let th = std::f32::consts::TAU * i as f32 / rest as f32;
        let r = 0.35 * ((i * 7) % 10) as f32 / 10.0;
        let z = if i % 2 == 0 { 0.5 } else { -0.5 };
        out.push(splat([r * th.cos(), z, r * th.sin()]));
    }
    out
}

fn encode(s: &[Splat]) -> Vec<f32> {
    let mut v = Vec::with_capacity(s.len() * PER);
    for g in s {
        v.extend_from_slice(&g.pos);
        v.push(g.scale);
        v.extend_from_slice(&g.color);
        v.push(g.alpha);
    }
    v
}

fn decode(v: &[f32]) -> Vec<Splat> {
    v.as_chunks::<PER>()
        .0
        .iter()
        .map(|c| Splat {
            pos: [c[0], c[1], c[2]],
            scale: c[3],
            color: [c[4], c[5], c[6]],
            alpha: c[7],
        })
        .collect()
}

/// 渲染「图 + 深度」：颜色走 `render_from`，深度按同一批权重加权平均。
fn render_rgb_depth(splats: &[Splat], cam: &Camera, res: usize) -> (Vec<f32>, Vec<f32>) {
    let cw = collect(splats, cam, res, res);
    let rgb = render_from(splats, &cw, res, res, [0.0; 3]);
    let mut dsum = vec![0.0f32; res * res];
    let mut wsum = vec![0.0f32; res * res];
    for &(i, p, w) in &cw.entries {
        if let Some((_, _, d)) = cam.project(splats[i as usize].pos) {
            dsum[p as usize] += w * d as f32;
            wsum[p as usize] += w;
        }
    }
    let depth = dsum
        .iter()
        .zip(&wsum)
        .map(|(a, b)| if *b > 1e-6 { a / b } else { 0.0 })
        .collect();
    (rgb, depth)
}

/// 图 + 深度 → 条件向量（8×8 深度均值 + 8×8 亮度）。
fn cond_of(rgb: &[f32], depth: &[f32], res: usize) -> Vec<f32> {
    let cell = res / G;
    let mut out = vec![0.0f32; COND];
    for gy in 0..G {
        for gx in 0..G {
            let mut ds = 0.0f32;
            let mut dc = 0usize;
            let mut ls = 0.0f32;
            for y in gy * cell..(gy + 1) * cell {
                for x in gx * cell..(gx + 1) * cell {
                    let p = y * res + x;
                    ds += depth[p];
                    if depth[p] > 0.0 {
                        dc += 1;
                    }
                    ls += 0.2126 * rgb[p * 3] + 0.7152 * rgb[p * 3 + 1] + 0.0722 * rgb[p * 3 + 2];
                }
            }
            out[gy * G + gx] = if dc > 0 { ds / dc as f32 } else { 0.0 };
            out[G * G + gy * G + gx] = ls / (cell * cell) as f32;
        }
    }
    out
}

/// 检索式占位场：按条件最近的模板，做 `v = k·(x − x₁)`。
struct SplatField {
    templates: Vec<Vec<f32>>,
    tconds: Vec<Vec<f32>>,
    k: f32,
}

impl Field for SplatField {
    fn dim(&self) -> usize {
        DIM
    }
    fn cond_dim(&self) -> usize {
        COND
    }
    fn eval(&self, x: &[f32], _t: &[f32], cond: &[f32], out: &mut [f32]) {
        for b in 0..cond.len() / COND {
            let c = &cond[b * COND..(b + 1) * COND];
            // 最近邻模板
            let mut best = 0usize;
            let mut best_d = f32::INFINITY;
            for (j, tc) in self.tconds.iter().enumerate() {
                let d: f32 = c.iter().zip(tc).map(|(a, b)| (a - b) * (a - b)).sum();
                if d < best_d {
                    best_d = d;
                    best = j;
                }
            }
            let tgt = &self.templates[best];
            for i in 0..DIM {
                out[b * DIM + i] = self.k * (x[b * DIM + i] - tgt[i]);
            }
        }
    }
}

fn l2(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y) * (x - y))
        .sum::<f32>()
        .sqrt()
}

fn main() {
    let templates = [sphere(), cube(), cylinder()];
    let encoded: Vec<Vec<f32>> = templates.iter().map(|t| encode(t)).collect();

    // 模板自己的条件（检索表的键）
    let tconds: Vec<Vec<f32>> = templates
        .iter()
        .map(|t| {
            let cam = orbit_camera([0.0; 3], 2.0, 15.0, 45.0, RES, 0.0);
            let (rgb, d) = render_rgb_depth(t, &cam, RES);
            cond_of(&rgb, &d, RES)
        })
        .collect();

    // 请求：每个形状 × 4 个视角；输入就是「图 + 深度」渲染出来的两张图
    let mut reqs = Vec::new();
    let mut want = Vec::new();
    for (ti, t) in templates.iter().enumerate() {
        for yaw in [0.0f32, 30.0, 60.0, 120.0] {
            let cam = orbit_camera([0.0; 3], 2.0, 15.0, 45.0, RES, yaw);
            let (rgb, d) = render_rgb_depth(t, &cam, RES);
            let c = cond_of(&rgb, &d, RES);
            let init: Vec<f32> = (0..DIM).map(|i| 0.5 + 0.3 * det(i + ti * DIM)).collect();
            reqs.push(Request::with_cond(init, c, STEPS, Scheme::Euler));
            want.push(ti);
        }
    }

    let field: Arc<dyn Field + Send + Sync> = Arc::new(SplatField {
        templates: encoded.clone(),
        tconds,
        k: 16.0,
    });

    println!("== 占位真实场：图 + 深度 → 3DGS splat ==");
    println!(
        "  形状 3 个 × 4 视角 = {} 请求；每请求 {} 高斯 × {} 参数 = {} 维；条件 {} 维",
        reqs.len(),
        N,
        PER,
        DIM,
        COND
    );
    println!();

    // 1) 正确性：终点必须回到目标模板
    let ex = Executor::new(
        Arc::clone(&field),
        Cfg {
            threads: 4,
            max_batch: 12,
            ..Cfg::default()
        },
    );
    let (out, rep) = ex.run(&reqs).expect("占位场");
    let mut worst = 0.0f32;
    for (i, o) in out.iter().enumerate() {
        let e = l2(&o.x, &encoded[want[i]]);
        worst = worst.max(e);
    }
    println!("正确性：最大 L2（终点 vs 目标模板）= {worst:.3e}（期望 < 1e-3）");
    println!(
        "  墙钟 {:.1} ms，{:.0} 样本·步/秒（{} 步 Euler）",
        rep.wall_ms, rep.sample_steps_per_s, STEPS
    );
    assert!(worst < 1e-3, "终点应回到目标模板，实测 L2 = {worst}");

    // 2) 写标准 3DGS PLY，再读回逐项对账
    let splats = decode(&out[0].x);
    let path = std::env::temp_dir().join("trit_flow_img23d.ply");
    write_ply_3dgs(&path, &splats).expect("写 PLY");
    let back = read_ply_3dgs(&path).expect("读回 PLY");
    assert_eq!(back.len(), splats.len(), "PLY 高斯数应一致");
    let mut rt = 0.0f32;
    for (a, b) in splats.iter().zip(&back) {
        rt = rt.max(
            (a.pos[0] - b.pos[0]).abs() + (a.pos[1] - b.pos[1]).abs() + (a.pos[2] - b.pos[2]).abs(),
        );
        rt = rt.max((a.scale - b.scale).abs());
        rt = rt.max((a.alpha - b.alpha).abs());
    }
    println!(
        "PLY 往返：{} 个高斯，最大逐项偏差 {rt:.3e}（{}）",
        back.len(),
        path.display()
    );
    assert!(rt < 1e-5, "PLY 往返应逐项一致，实测偏差 {rt}");

    // 3) 并发读数（同一场、同一批请求）
    println!();
    println!(
        "{:<22} {:>8} {:>8} {:>8} {:>9} {:>12} {:>7}",
        "配置", "p50/ms", "p90/ms", "p99/ms", "req/s", "样本·步/秒", "平均批"
    );
    for (name, cfg) in [
        (
            "串行 batch=1",
            Cfg {
                threads: 0,
                max_batch: 1,
                ..Cfg::default()
            },
        ),
        (
            "批 batch=12",
            Cfg {
                threads: 0,
                max_batch: 12,
                ..Cfg::default()
            },
        ),
        (
            "批+并行 8 线程",
            Cfg {
                threads: 8,
                max_batch: 12,
                ..Cfg::default()
            },
        ),
    ] {
        let ex = Executor::new(Arc::clone(&field), cfg);
        let (_, r) = ex.run(&reqs).expect("读数");
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
    println!();
    println!("注：这是占位场（检索式，未训练）；换真权重只换 SplatField::eval 的内部。");
}

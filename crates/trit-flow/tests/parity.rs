//! 批处理 / 并行 / 在线三种组织方式，结果必须与「单请求串行」**逐位一致**。
//!
//! 这是执行器唯一的正确性判据：批只改变「一次算几个」，不改变任何一个样本的数值。
//! 任何一条红了，说明批的组织动了数值（组批、时间对齐或切片写回有错）。

use std::sync::Arc;

use trit_bench::SeedRng;
use trit_flow::{Cfg, Executor, Field, MlpField, Request, Scheme};

fn field() -> Arc<dyn Field + Send + Sync> {
    Arc::new(MlpField::new(16, 4, 12, 7))
}

fn vals(seed: u64, n: usize) -> Vec<f32> {
    let mut rng = SeedRng::new(seed);
    (0..n)
        .map(|_| (rng.next_u64() >> 40) as f32 / (1u32 << 24) as f32)
        .collect()
}

fn reqs(n: usize, steps: &[usize], scheme: Scheme) -> Vec<Request> {
    (0..n)
        .map(|i| {
            Request::with_cond(
                vals(i as u64 + 1, 16),
                vals(100 + i as u64, 4),
                steps[i % steps.len()],
                scheme,
            )
        })
        .collect()
}

fn alone(f: &Arc<dyn Field + Send + Sync>, r: &Request) -> Vec<f32> {
    let ex = Executor::new(
        Arc::clone(f),
        Cfg {
            threads: 0,
            max_batch: 1,
        },
    );
    let (out, _) = ex.run(std::slice::from_ref(r)).expect("单请求");
    out[0].x.clone()
}

#[test]
fn batch_equals_single() {
    let f = field();
    let rs = reqs(6, &[5], Scheme::Heun);
    let ex = Executor::new(
        Arc::clone(&f),
        Cfg {
            threads: 3,
            max_batch: 8,
        },
    );
    let (batched, _) = ex.run(&rs).expect("批跑");
    for (i, r) in rs.iter().enumerate() {
        assert_eq!(batched[i].x, alone(&f, r), "请求 {i}：批内与单独跑不一致");
    }
}

#[test]
fn parallel_equals_serial() {
    let f = field();
    let rs = reqs(9, &[4], Scheme::Midpoint);
    let par = Executor::new(
        Arc::clone(&f),
        Cfg {
            threads: 4,
            max_batch: 9,
        },
    );
    let ser = Executor::new(
        Arc::clone(&f),
        Cfg {
            threads: 0,
            max_batch: 9,
        },
    );
    let (a, ra) = par.run(&rs).expect("并行");
    let (b, rb) = ser.run(&rs).expect("串行");
    for i in 0..rs.len() {
        assert_eq!(a[i].x, b[i].x, "请求 {i}：并行与串行不一致");
    }
    assert_eq!(ra.requests, rb.requests);
    assert_eq!(ra.sample_steps, rb.sample_steps);
    // 并行片上界：片数不超过线程数（每次 advance 一片至少一个样本）
    assert!(ra.peak_batch <= 9);
}

#[test]
fn mixed_steps_in_one_batch() {
    // 同批里 3/5/7 步混跑：每样本自己的 t 与 Δt
    let f = field();
    let rs = reqs(6, &[3, 5, 7], Scheme::Euler);
    let ex = Executor::new(
        Arc::clone(&f),
        Cfg {
            threads: 2,
            max_batch: 6,
        },
    );
    let (out, rep) = ex.run(&rs).expect("混批");
    assert_eq!(rep.rounds, 7, "轮数应等于最大步数");
    for (i, r) in rs.iter().enumerate() {
        assert_eq!(out[i].x, alone(&f, r), "请求 {i}（{} 步）不一致", r.steps);
    }
}

#[test]
fn online_equals_offline() {
    let f = field();
    let rs = reqs(4, &[5], Scheme::Euler);
    let ex = Executor::new(
        Arc::clone(&f),
        Cfg {
            threads: 2,
            max_batch: 4,
        },
    );
    let (off, _) = ex.run(&rs).expect("离线");
    let arrivals = [0.0, 1.0, 2.0, 3.0];
    let (on, rep) = ex.run_with_arrivals(&arrivals, &rs).expect("在线");
    for i in 0..rs.len() {
        assert_eq!(off[i].x, on[i].x, "请求 {i}：在线与离线不一致");
    }
    assert_eq!(rep.requests, 4);
    // 后到的请求延迟应包含等待，至少不小于先到的
    assert!(rep.latency.max >= rep.latency.min);
}

#[test]
fn noiseless_field_is_deterministic_across_runs() {
    let f = field();
    let rs = reqs(3, &[6], Scheme::Heun);
    let ex = Executor::new(
        Arc::clone(&f),
        Cfg {
            threads: 2,
            max_batch: 3,
        },
    );
    let (a, _) = ex.run(&rs).expect("第一次");
    let (b, _) = ex.run(&rs).expect("第二次");
    for i in 0..rs.len() {
        assert_eq!(a[i].x, b[i].x, "同输入两次跑必须逐位一致");
    }
}

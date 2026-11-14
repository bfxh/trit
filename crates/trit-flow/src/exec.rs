//! 批量执行器：常驻线程池 + 按步组批 + 延迟/吞吐读数。
//!
//! **组织方式**（这是「高并发秒级」的内核）：
//! - 每个请求自己带进度 `cur[i]` 与步数 `steps[i]`，每轮各推进一步；
//!   所以**不同步数的请求在同一批里推进**（`t` 与 `Δt` 按样本给，见 [`crate::Field`]）；
//! - 每轮按积分方案分组，再按 `max_batch` 切块——块就是一次 `Pool::advance`；
//! - 在线模式只是把「到达时刻」变成激活条件：轮与轮之间到期的请求当轮加入，从 `t = 1` 起步；
//!   离线模式是「全部在 0 ms 到达」的特例，走**同一条**代码路径。
//!
//! 口径：延迟 = 请求从进入执行器（到达）到完成最后一轮的墙钟毫秒；吞吐 = 请求数 / 墙钟秒。

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::field::{Field, ShapeError};
use crate::pool::Pool;
use crate::solver::Scheme;
use crate::stats::{Latency, summarize};

/// 执行器配置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cfg {
    /// 工作线程数（0 = 主线程直接算，用于对照）。
    pub threads: usize,
    /// 单批样本上限（一次 `Pool::advance` 最多几个样本）。
    pub max_batch: usize,
    /// 末步是否调用 [`Field::terminal_snap`]（扩散侧按 DDIM 惯例直接读 `x̂₀`）。
    ///
    /// 纯速度场对该开关不敏感（trait 默认返回 `false`），所以默认开是安全的。
    pub snap: bool,
}

impl Default for Cfg {
    fn default() -> Self {
        let threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
        Self {
            threads,
            max_batch: 32,
            snap: true,
        }
    }
}

/// 一个采样请求。
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    /// 初始状态（噪声），长度 = `field.dim()`。
    pub init: Vec<f32>,
    /// 条件，长度 = `field.cond_dim()`。
    pub cond: Vec<f32>,
    /// 步数（≥ 1）。
    pub steps: usize,
    /// 积分方案。
    pub scheme: Scheme,
}

impl Request {
    /// 无条件请求。
    #[must_use]
    pub fn new(init: Vec<f32>, steps: usize, scheme: Scheme) -> Self {
        Self {
            init,
            cond: Vec::new(),
            steps,
            scheme,
        }
    }

    /// 带条件请求。
    #[must_use]
    pub fn with_cond(init: Vec<f32>, cond: Vec<f32>, steps: usize, scheme: Scheme) -> Self {
        Self {
            init,
            cond,
            steps,
            scheme,
        }
    }
}

/// 单个请求的结果。
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    /// 终态（数据侧），长度 = `field.dim()`。
    pub x: Vec<f32>,
    /// 完成的墙钟毫秒（相对执行器开始）。
    pub done_ms: f64,
}

/// 一次运行的汇总读数。
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    /// 请求数。
    pub requests: usize,
    /// 延迟读数（毫秒）。
    pub latency: Latency,
    /// 墙钟总时长（毫秒）。
    pub wall_ms: f64,
    /// 吞吐：请求/秒。
    pub rps: f64,
    /// 累计样本·步。
    pub sample_steps: u64,
    /// 吞吐：样本·步/秒（跨步数配置可比）。
    pub sample_steps_per_s: f64,
    /// 场求值次数。
    pub field_evals: u64,
    /// 推进轮数。
    pub rounds: u64,
    /// 最大单批样本数。
    pub peak_batch: usize,
    /// 平均单批样本数。
    pub mean_batch: f64,
    /// 工作线程数。
    pub threads: usize,
    /// 单批上限。
    pub max_batch: usize,
}

impl Report {
    fn empty(cfg: Cfg) -> Self {
        Self {
            requests: 0,
            latency: Latency {
                n: 0,
                min: 0.0,
                p50: 0.0,
                p90: 0.0,
                p99: 0.0,
                max: 0.0,
                mean: 0.0,
            },
            wall_ms: 0.0,
            rps: 0.0,
            sample_steps: 0,
            sample_steps_per_s: 0.0,
            field_evals: 0,
            rounds: 0,
            peak_batch: 0,
            mean_batch: 0.0,
            threads: cfg.threads,
            max_batch: cfg.max_batch,
        }
    }
}

/// 批量执行器：持有一个场（权重常驻）与一组常驻线程池。
pub struct Executor {
    field: Arc<dyn Field + Send + Sync>,
    cfg: Cfg,
}

impl Executor {
    /// 建执行器（不启线程，第一次用到某个方案时才建池）。
    #[must_use]
    pub fn new(field: Arc<dyn Field + Send + Sync>, cfg: Cfg) -> Self {
        Self { field, cfg }
    }

    /// 配置。
    #[must_use]
    pub fn cfg(&self) -> Cfg {
        self.cfg
    }

    /// 离线跑一批（全部 0 ms 到达）。
    ///
    /// # Errors
    /// 任一请求形状不合法。
    pub fn run(&self, reqs: &[Request]) -> Result<(Vec<Outcome>, Report), ShapeError> {
        let arrivals = vec![0.0; reqs.len()];
        self.run_with_arrivals(&arrivals, reqs)
    }

    /// 在线跑：`arrive_ms[i]` 是第 `i` 个请求进入执行器的时刻（毫秒，非递减）。
    ///
    /// 到达时刻之后的轮次才把它算进批里；到达前的轮次空转或睡眠。
    ///
    /// # Errors
    /// 形状不合法，或到达时刻不是非递减。
    pub fn run_with_arrivals(
        &self,
        arrive_ms: &[f64],
        reqs: &[Request],
    ) -> Result<(Vec<Outcome>, Report), ShapeError> {
        let dim = self.field.dim();
        let cd = self.field.cond_dim();
        if arrive_ms.len() != reqs.len() {
            return Err(ShapeError {
                what: "到达时刻数",
                got: arrive_ms.len(),
                want: reqs.len(),
            });
        }
        for r in reqs {
            if r.steps == 0 {
                return Err(ShapeError {
                    what: "步数（必须 ≥ 1）",
                    got: 0,
                    want: 1,
                });
            }
            if r.init.len() != dim {
                return Err(ShapeError {
                    what: "init 长度",
                    got: r.init.len(),
                    want: dim,
                });
            }
            if r.cond.len() != cd {
                return Err(ShapeError {
                    what: "cond 长度",
                    got: r.cond.len(),
                    want: cd,
                });
            }
        }
        for w in arrive_ms.windows(2) {
            if w[1] < w[0] {
                return Err(ShapeError {
                    what: "到达时刻必须非递减",
                    got: 1,
                    want: 0,
                });
            }
        }

        let n = reqs.len();
        if n == 0 {
            return Ok((Vec::new(), Report::empty(self.cfg)));
        }

        // 状态
        let mut x = vec![0.0f32; n * dim];
        for (i, r) in reqs.iter().enumerate() {
            x[i * dim..(i + 1) * dim].copy_from_slice(&r.init);
        }
        let mut cur = vec![0usize; n];
        let mut active = vec![false; n];
        let mut done_ms = vec![0.0f64; n];

        // 按到达时刻排序的激活顺序（稳定：同刻按输入序）。
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_by(|&a, &b| {
            arrive_ms[a]
                .partial_cmp(&arrive_ms[b])
                .expect("到达时刻不应含 NaN")
                .then(a.cmp(&b))
        });
        let mut next_arrival = 0usize;

        let mut pools: [Option<Pool>; 3] = [None, None, None];
        let cap = self.cfg.max_batch.min(n).max(1);
        let mut buf = vec![0.0f32; cap * dim];
        let mut bt = vec![0.0f32; cap];
        let mut bdt = vec![0.0f32; cap];
        let mut bcond = vec![0.0f32; cap * cd];
        // 末步 snap 的临时缓冲（复用，不每轮分配）
        let mut snap_buf = vec![0.0f32; cap * dim];
        let mut snap_out = vec![0.0f32; cap * dim];
        let mut snap_cond = vec![0.0f32; cap * cd];
        let mut snap_t = vec![0.0f32; cap];
        let mut snap_rows: Vec<usize> = Vec::with_capacity(cap);

        let start = Instant::now();
        let mut rounds = 0u64;
        let mut evals = 0u64;
        let mut batch_samples = 0u64;
        let mut batches = 0u64;
        let mut peak_batch = 0usize;

        loop {
            let now_ms = start.elapsed().as_secs_f64() * 1000.0;
            // 吸收已到期请求
            while next_arrival < n && arrive_ms[order[next_arrival]] <= now_ms {
                active[order[next_arrival]] = true;
                next_arrival += 1;
            }
            if !active.iter().any(|&a| a) {
                if next_arrival >= n {
                    break; // 全部完成
                }
                let wait = arrive_ms[order[next_arrival]] - now_ms;
                if wait > 0.0 {
                    std::thread::sleep(Duration::from_secs_f64(wait / 1000.0));
                }
                continue;
            }

            // 每个方案各推进一步
            for scheme in [Scheme::Euler, Scheme::Midpoint, Scheme::Heun] {
                let idx: Vec<usize> = (0..n)
                    .filter(|&i| active[i] && reqs[i].scheme == scheme)
                    .collect();
                if idx.is_empty() {
                    continue;
                }
                let pool = pools[scheme as usize].get_or_insert_with(|| {
                    Pool::new(Arc::clone(&self.field), scheme, self.cfg.threads)
                });
                for chunk in idx.chunks(self.cfg.max_batch) {
                    let m = chunk.len();
                    for (j, &i) in chunk.iter().enumerate() {
                        let s = reqs[i].steps;
                        let frac = cur[i] as f32 / s as f32;
                        buf[j * dim..(j + 1) * dim].copy_from_slice(&x[i * dim..(i + 1) * dim]);
                        bt[j] = 1.0 - frac;
                        bdt[j] = -1.0 / s as f32;
                        if cd > 0 {
                            bcond[j * cd..(j + 1) * cd].copy_from_slice(&reqs[i].cond);
                        }
                    }
                    evals += pool.advance(
                        &mut buf[..m * dim],
                        &bt[..m],
                        &bdt[..m],
                        &bcond[..m * cd],
                        m,
                    );
                    for (j, &i) in chunk.iter().enumerate() {
                        x[i * dim..(i + 1) * dim].copy_from_slice(&buf[j * dim..(j + 1) * dim]);
                        cur[i] += 1;
                    }
                    // 末步 snap：扩散侧 `t→0` 被 1/√(1−ᾱ) 放大，继续积分只是放大误差，
                    // 按 DDIM 惯例直接读 x̂₀。纯速度场的 `terminal_snap` 返回 false ⇒ 无动作。
                    if self.cfg.snap {
                        snap_rows.clear();
                        for &i in chunk {
                            if cur[i] == reqs[i].steps {
                                let k = snap_rows.len();
                                snap_buf[k * dim..(k + 1) * dim]
                                    .copy_from_slice(&x[i * dim..(i + 1) * dim]);
                                if cd > 0 {
                                    snap_cond[k * cd..(k + 1) * cd].copy_from_slice(&reqs[i].cond);
                                }
                                snap_t[k] = 0.0;
                                snap_rows.push(i);
                            }
                        }
                        let k = snap_rows.len();
                        if k > 0
                            && self.field.terminal_snap(
                                &snap_buf[..k * dim],
                                &snap_t[..k],
                                &snap_cond[..k * cd],
                                &mut snap_out[..k * dim],
                            )
                        {
                            for (kk, &i) in snap_rows.iter().enumerate() {
                                x[i * dim..(i + 1) * dim]
                                    .copy_from_slice(&snap_out[kk * dim..(kk + 1) * dim]);
                            }
                        }
                    }
                    batch_samples += m as u64;
                    batches += 1;
                    peak_batch = peak_batch.max(m);
                }
            }

            let done_now = start.elapsed().as_secs_f64() * 1000.0;
            for i in 0..n {
                if active[i] && cur[i] >= reqs[i].steps {
                    active[i] = false;
                    done_ms[i] = done_now;
                }
            }
            rounds += 1;
            if rounds as usize > 4 * n + 4 * reqs.iter().map(|r| r.steps).sum::<usize>() + 16 {
                // 保险丝：每个请求每轮最多推进一步，正常永远到不了这里。
                panic!(
                    "执行器轮数超界：cur={cur:?} steps={:?}",
                    reqs.iter().map(|r| r.steps).collect::<Vec<_>>()
                );
            }
            if next_arrival >= n && !active.iter().any(|&a| a) {
                break;
            }
        }

        let wall_ms = start.elapsed().as_secs_f64() * 1000.0;
        // 零延迟样本（极短请求）按实际墙钟兜底，避免把 p50 报成 0。
        let mut lat = done_ms.clone();
        for v in &mut lat {
            if *v == 0.0 {
                *v = wall_ms;
            }
        }
        let latency = summarize(&lat);
        let total_steps: u64 = reqs.iter().map(|r| r.steps as u64).sum();

        let outcomes: Vec<Outcome> = (0..n)
            .map(|i| Outcome {
                x: x[i * dim..(i + 1) * dim].to_vec(),
                done_ms: done_ms[i],
            })
            .collect();
        let report = Report {
            requests: n,
            latency,
            wall_ms,
            rps: n as f64 / (wall_ms / 1000.0).max(f64::MIN_POSITIVE),
            sample_steps: total_steps,
            sample_steps_per_s: total_steps as f64 / (wall_ms / 1000.0).max(f64::MIN_POSITIVE),
            field_evals: evals,
            rounds,
            peak_batch,
            mean_batch: if batches == 0 {
                0.0
            } else {
                batch_samples as f64 / batches as f64
            },
            threads: self.cfg.threads,
            max_batch: self.cfg.max_batch,
        };
        Ok((outcomes, report))
    }
}

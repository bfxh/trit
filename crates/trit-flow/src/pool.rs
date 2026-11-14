//! 常驻工作线程池：把一批样本切成片并行推进。
//!
//! **为什么不用 rayon / tokio**：本仓 MSRV 1.88 且底座层不拉外部运行时（见 `docs/LAYERS.md`），
//! `std::thread` + `mpsc` 足够表达「片间无共享、算完回传」这一种并行。
//!
//! **口径**：切片只按样本行切，片与片之间**没有**任何依赖，所以结果与单线程逐位一致
//! （`tests/batch_parity.rs` 钉住）。片数据拷贝进出是显式成本，不藏。

use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use crate::field::Field;
use crate::solver::{Scheme, Scratch, advance};

/// 一片的算力请求。
struct Job {
    /// 片号（写回位置）。
    id: usize,
    /// `rows × dim` 行主序状态。
    x: Vec<f32>,
    /// 每样本当前时间。
    t: Vec<f32>,
    /// 每样本步长。
    dt: Vec<f32>,
    /// `rows × cond_dim`。
    cond: Vec<f32>,
}

/// 常驻线程池。建一次、跑多批——对应服务里的「进程常驻 + 权重常驻」。
pub struct Pool {
    jobs: Vec<Sender<Job>>,
    done: Receiver<(usize, Vec<f32>)>,
    threads: Vec<JoinHandle<()>>,
    local: Mutex<Scratch>,
    field: Arc<dyn Field + Send + Sync>,
    scheme: Scheme,
    dim: usize,
    cond_dim: usize,
}

impl Pool {
    /// 建池：`workers` 个常驻线程共享同一个场（`Arc`，权重常驻）。
    ///
    /// `workers == 0` 时退化成「主线程直接算」，用于对照读数。
    pub fn new(field: Arc<dyn Field + Send + Sync>, scheme: Scheme, workers: usize) -> Self {
        let dim = field.dim();
        let cond_dim = field.cond_dim();
        let (done_tx, done) = channel::<(usize, Vec<f32>)>();
        let mut jobs = Vec::with_capacity(workers);
        let mut threads = Vec::with_capacity(workers);
        for _ in 0..workers {
            let (tx, rx) = channel::<Job>();
            let f = Arc::clone(&field);
            let res = done_tx.clone();
            threads.push(thread::spawn(move || {
                let mut scratch = Scratch::new();
                while let Ok(job) = rx.recv() {
                    let mut x = job.x;
                    advance(
                        f.as_ref(),
                        scheme,
                        &mut x,
                        &job.t,
                        &job.dt,
                        &job.cond,
                        &mut scratch,
                    );
                    if res.send((job.id, x)).is_err() {
                        break;
                    }
                }
            }));
            jobs.push(tx);
        }
        drop(done_tx);
        Self {
            jobs,
            done,
            threads,
            local: Mutex::new(Scratch::new()),
            field,
            scheme,
            dim,
            cond_dim,
        }
    }

    /// 工作线程数。
    #[must_use]
    pub fn workers(&self) -> usize {
        self.threads.len()
    }

    /// 本池绑定的积分方案。
    #[must_use]
    pub fn scheme(&self) -> Scheme {
        self.scheme
    }

    /// 推进一批样本一步：`x` 是 `n × dim`，原地更新。
    ///
    /// 返回这次推进消耗的**场求值次数**（= 片数 × `scheme.evals_per_step()`）。
    ///
    /// # Panics
    /// 形状与池不一致。
    pub fn advance(&self, x: &mut [f32], t: &[f32], dt: &[f32], cond: &[f32], n: usize) -> u64 {
        assert_eq!(x.len(), n * self.dim, "x 长度必须是 n×dim");
        assert_eq!(t.len(), n, "t 长度必须是 n");
        assert_eq!(dt.len(), n, "dt 长度必须是 n");
        assert_eq!(cond.len(), n * self.cond_dim, "cond 长度必须是 n×cond_dim");
        if n == 0 {
            return 0;
        }
        let per_call = self.scheme.evals_per_step() as u64;
        if self.workers() == 0 || n == 1 {
            let mut scratch = self.local.lock().expect("本地缓冲锁");
            advance(
                self.field.as_ref(),
                self.scheme,
                &mut x[..n * self.dim],
                &t[..n],
                &dt[..n],
                &cond[..n * self.cond_dim],
                &mut scratch,
            );
            return per_call;
        }

        let pieces = self.workers().min(n);
        let per = n.div_ceil(pieces);
        let starts: Vec<usize> = (0..n).step_by(per).collect();
        let count = starts.len();
        let (dim, cd) = (self.dim, self.cond_dim);

        for (id, &start) in starts.iter().enumerate() {
            let end = (start + per).min(n);
            if id + 1 == count {
                // 最后一片留在主线程：省一次往返，也让 `workers=1` 有意义。
                let mut scratch = self.local.lock().expect("本地缓冲锁");
                advance(
                    self.field.as_ref(),
                    self.scheme,
                    &mut x[start * dim..end * dim],
                    &t[start..end],
                    &dt[start..end],
                    &cond[start * cd..end * cd],
                    &mut scratch,
                );
            } else {
                let job = Job {
                    id,
                    x: x[start * dim..end * dim].to_vec(),
                    t: t[start..end].to_vec(),
                    dt: dt[start..end].to_vec(),
                    cond: cond[start * cd..end * cd].to_vec(),
                };
                self.jobs[id]
                    .send(job)
                    .expect("工作线程不应提前退出（池由执行器持有）");
            }
        }

        for _ in 0..count - 1 {
            let (id, data) = self.done.recv().expect("工作线程不应提前退出");
            let start = starts[id];
            let end = (start + per).min(n);
            x[start * dim..end * dim].copy_from_slice(&data);
        }

        count as u64 * per_call
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        self.jobs.clear();
        for h in self.threads.drain(..) {
            let _ = h.join();
        }
    }
}

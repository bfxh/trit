//! 自旋线程池。
//!
//! decode 每 token 有上百次小矩阵向量乘，每次派发都唤醒 worker 的话，
//! 调度开销会盖过收益。这里用 epoch + 自旋，单次派发约 1 µs。
//!
//! 实测依据：同机 SmolLM2-135M，rayon 逐 matvec 派发 1 线程 10.54 →
//! 24 线程 7.60 tok/s（负收益）；换成本池后 1 线程 49.75 → 8 线程 79.9。

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

type Job = Arc<dyn Fn(usize) + Send + Sync>;

/// 固定大小的自旋线程池。
pub struct Pool {
    n: usize,
    epoch: AtomicU64,
    pending: AtomicUsize,
    /// 任一 worker 的作业 panic 时置位；`run` 收尾后据此向调用方传播（不让 panic 静默消失）。
    panicked: AtomicBool,
    job: Mutex<Option<Job>>,
    handles: Mutex<Vec<std::thread::JoinHandle<()>>>,
    /// `run` 不可重入：多个调用方同时派发会互相覆盖 job 槽。
    /// 加锁把并发派发串行化（无竞争时约 20 ns）。
    run_lock: Mutex<()>,
}

impl Pool {
    /// 建池；`n` 个线程（含调用方主线程）。
    pub fn new(n: usize) -> Arc<Self> {
        let n = n.max(1);
        let pool = Arc::new(Pool {
            n,
            epoch: AtomicU64::new(0),
            pending: AtomicUsize::new(0),
            panicked: AtomicBool::new(false),
            job: Mutex::new(None),
            handles: Mutex::new(Vec::new()),
            run_lock: Mutex::new(()),
        });
        for k in 1..n {
            let me = Arc::clone(&pool);
            let handle = std::thread::spawn(move || {
                let mut seen = 0u64;
                loop {
                    loop {
                        let e = me.epoch.load(Ordering::Acquire);
                        if e != seen {
                            seen = e;
                            break;
                        }
                        std::hint::spin_loop();
                    }
                    let job = me.job.lock().unwrap().clone();
                    if let Some(f) = job {
                        // worker panic 不能杀死线程、也不能跳过 pending 递减：否则主线程在
                        // `run` 里永久自旋，且池永久少一个 worker。catch 住、记标志、照常递减。
                        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(k))).is_err()
                        {
                            me.panicked.store(true, Ordering::Release);
                        }
                    }
                    me.pending.fetch_sub(1, Ordering::Release);
                }
            });
            pool.handles.lock().unwrap().push(handle);
        }
        pool
    }

    /// 线程数。
    #[inline]
    pub fn threads(&self) -> usize {
        self.n
    }

    /// 把 `0..n` 分给各线程并等待全部完成。
    pub fn run<F>(&self, f: F)
    where
        F: Fn(usize) + Send + Sync + 'static,
    {
        if self.n == 1 {
            f(0);
            return;
        }
        // 把「派发 + 收尾」放进内层块：`_serialize` 在此正常 drop，绝不在持锁时 unwind
        //（否则 run_lock 中毒，之后每次 run() 都在 `lock().unwrap()` 处 panic = 池永久失效）。
        let outcome = {
            let _serialize = self.run_lock.lock().unwrap();
            self.panicked.store(false, Ordering::Release);
            *self.job.lock().unwrap() = Some(Arc::new(f));
            self.pending.store(self.n - 1, Ordering::Release);
            self.epoch.fetch_add(1, Ordering::Release);
            let job = self.job.lock().unwrap().clone().unwrap();
            // 主线程 panic 也必须先让 worker 全部结束：否则调用方的缓冲已随 unwind 释放，
            // worker 仍在通过裸指针写它 = use-after-free。先 catch，等齐 pending，清 job，
            // 到块外再 resume（此时已不持 run_lock，不会中毒）。
            let main = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(0)));
            while self.pending.load(Ordering::Acquire) != 0 {
                std::hint::spin_loop();
            }
            *self.job.lock().unwrap() = None;
            (main, self.panicked.load(Ordering::Acquire))
        };
        if let Err(payload) = outcome.0 {
            std::panic::resume_unwind(payload);
        }
        if outcome.1 {
            panic!("线程池任务 panic（worker 已捕获，池状态已一致，可继续使用）");
        }
    }
}

static GLOBAL: OnceLock<Arc<Pool>> = OnceLock::new();
static THREADS: AtomicUsize = AtomicUsize::new(8);

/// 设置全局池线程数（首次取池前生效）。
pub fn set_threads(n: usize) {
    THREADS.store(n.max(1), Ordering::Relaxed);
}

/// 取全局池。
pub fn global() -> &'static Pool {
    GLOBAL.get_or_init(|| Pool::new(THREADS.load(Ordering::Relaxed)))
}

/// 裸指针包装：让闭包满足 `Send + Sync`。
///
/// 正确性由 [`Pool::run`] 的完整屏障保证——返回前所有 worker 都已结束。
#[derive(Clone, Copy)]
pub struct Ptr<T>(*mut T);

// SAFETY: 只在 `par_rows_mut` 内使用，每个 worker 只访问自己那一段。
unsafe impl<T> Send for Ptr<T> {}
// SAFETY: 同上；切片被按行切分，无重叠写。
unsafe impl<T> Sync for Ptr<T> {}

impl<T> Ptr<T> {
    /// 包装裸指针。
    #[inline]
    pub fn new(ptr: *mut T) -> Self {
        Ptr(ptr)
    }

    /// 取裸指针。
    #[inline]
    pub fn get(&self) -> *mut T {
        self.0
    }
}

/// 按行分给池里的线程，每个线程拿到自己那一段的裸指针。
///
/// # Safety
/// `ptr` 必须指向至少 `len` 个有效元素；`f` 在不同行之间不得产生重叠写。
pub unsafe fn par_rows_mut<T: 'static, F>(ptr: *mut T, len: usize, f: F)
where
    F: Fn(*mut T, usize) + Send + Sync + 'static,
{
    let pool = global();
    let n = pool.threads();
    if n == 1 || len < 32 {
        for r in 0..len {
            // SAFETY: 由调用方保证。
            unsafe { f(ptr.add(r), r) };
        }
        return;
    }
    let base = Ptr(ptr);
    let chunk = len.div_ceil(n);
    pool.run(move |k| {
        let start = k * chunk;
        let end = ((k + 1) * chunk).min(len);
        for r in start..end {
            // SAFETY: 各 k 的 [start, end) 互不相交。
            unsafe { f(base.get().add(r), r) };
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn every_row_visited_exactly_once() {
        let pool = Pool::new(4);
        let len = 1000usize;
        let mut hits = vec![0u32; len];
        let base = Ptr(hits.as_mut_ptr());
        pool.run(move |k| {
            let chunk = len.div_ceil(4);
            let start = k * chunk;
            let end = ((k + 1) * chunk).min(len);
            for r in start..end {
                // SAFETY: 区间互不相交。
                unsafe { *base.get().add(r) += 1 };
            }
        });
        assert!(hits.iter().all(|&h| h == 1));
    }

    #[test]
    fn single_thread_pool_still_works() {
        let pool = Pool::new(1);
        let counter = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&counter);
        pool.run(move |_| {
            c.fetch_add(1, Ordering::Relaxed);
        });
        assert_eq!(counter.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn main_thread_panic_propagates_but_waits_for_workers_and_pool_survives() {
        let pool = Pool::new(4);
        let hit = Arc::new(AtomicUsize::new(0));
        let h = Arc::clone(&hit);
        // k==0 是主线程：让它 panic。worker（k=1..4）仍必须跑完各自的段。
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            pool.run(move |k| {
                if k == 0 {
                    panic!("main boom");
                }
                h.fetch_add(1, Ordering::Relaxed);
            });
        }));
        assert!(res.is_err(), "主线程 panic 必须传播给调用方");
        // 传播前必须等齐 pending ⇒ 返回时 3 个 worker 段都跑完（否则缓冲已释放、worker 仍在写 = UAF）。
        assert_eq!(
            hit.load(Ordering::Relaxed),
            3,
            "worker 未跑完（未等齐 pending = UAF）"
        );
        // run_lock 不能被上一次 panic 中毒：再跑一次必须成功。
        let second = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            pool.run(|k| {
                let _ = k;
            });
        }));
        assert!(second.is_ok(), "上一次 panic 毒死了 run_lock（池永久失效）");
    }

    #[test]
    fn worker_panic_does_not_hang_and_pool_survives() {
        use std::sync::mpsc;
        use std::time::Duration;
        // 子线程 + recv_timeout：worker panic 若让主线程永久自旋，会在这里超时成断言失败
        //（不挂死测试、变异门也能判「抓住」而非「超时」）。
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let pool = Pool::new(4);
            let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                pool.run(|k| {
                    if k == 2 {
                        panic!("worker boom");
                    }
                });
            }));
            // worker panic 应转为调用方可见的 panic，且池仍可用（worker 线程没死）。
            let promoted = res.is_err();
            let reusable = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                pool.run(|k| {
                    let _ = k;
                });
            }))
            .is_ok();
            let _ = tx.send(promoted && reusable);
        });
        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(v) => assert!(v, "worker panic 未传播或池失效"),
            Err(_) => panic!("worker panic 后 Pool::run 挂死（永久自旋回归）"),
        }
    }
}

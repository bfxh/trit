//! 自旋线程池。
//!
//! decode 每 token 有上百次小矩阵向量乘，每次派发都唤醒 worker 的话，
//! 调度开销会盖过收益。这里用 epoch + 自旋，单次派发约 1 µs。
//!
//! 实测依据：同机 SmolLM2-135M，rayon 逐 matvec 派发 1 线程 10.54 →
//! 24 线程 7.60 tok/s（负收益）；换成本池后 1 线程 49.75 → 8 线程 79.9。

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

type Job = Arc<dyn Fn(usize) + Send + Sync>;

/// 固定大小的自旋线程池。
pub struct Pool {
    n: usize,
    epoch: AtomicU64,
    pending: AtomicUsize,
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
                        f(k);
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
        let _serialize = self.run_lock.lock().unwrap();
        *self.job.lock().unwrap() = Some(Arc::new(f));
        self.pending.store(self.n - 1, Ordering::Release);
        self.epoch.fetch_add(1, Ordering::Release);
        let job = self.job.lock().unwrap().clone().unwrap();
        job(0);
        while self.pending.load(Ordering::Acquire) != 0 {
            std::hint::spin_loop();
        }
        *self.job.lock().unwrap() = None;
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
}

//! pool 的单元测试（从 `src/pool.rs` 拆出，控制单文件行数预算）。

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

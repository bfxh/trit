//! hlc 的单元测试（从 src/hlc.rs 拆出，控制单文件行数预算）。

use super::*;

fn fixed(ms: u64) -> Box<dyn Fn() -> u64 + Send + Sync> {
    Box::new(move || ms)
}

#[test]
fn ticks_are_strictly_increasing() {
    let mut c = Clock::with_now(1, fixed(1000));
    let mut prev = None;
    for i in 0..5u32 {
        let t = c.tick();
        assert_eq!(t.physical_ms, 1000);
        assert_eq!(t.counter, i);
        assert_eq!(t.writer, 1);
        if let Some(p) = prev {
            assert!(t > p);
        }
        prev = Some(t);
    }
}

#[test]
fn physical_jump_forward_resets_counter() {
    let ms = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(1000));
    let ms2 = std::sync::Arc::clone(&ms);
    let mut c = Clock::with_now(
        1,
        Box::new(move || ms2.load(std::sync::atomic::Ordering::Relaxed)),
    );
    let t1 = c.tick();
    ms.store(2000, std::sync::atomic::Ordering::Relaxed);
    let t2 = c.tick();
    assert_eq!(t2.physical_ms, 2000);
    assert_eq!(t2.counter, 0);
    assert!(t2 > t1);
}

#[test]
fn physical_backwards_keeps_causality() {
    let ms = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(1000));
    let ms2 = std::sync::Arc::clone(&ms);
    let mut c = Clock::with_now(
        1,
        Box::new(move || ms2.load(std::sync::atomic::Ordering::Relaxed)),
    );
    let t1 = c.tick();
    ms.store(500, std::sync::atomic::Ordering::Relaxed);
    let t2 = c.tick();
    assert_eq!(t2.physical_ms, 1000, "回拨不回退物理位");
    assert!(t2 > t1);
}

#[test]
fn observe_makes_future_ticks_causally_later() {
    let mut a = Clock::with_now(1, fixed(1000));
    let mut b = Clock::with_now(2, fixed(1000));
    let ta = a.tick();
    b.observe(&ta);
    let tb = b.tick();
    assert!(tb > ta, "观察到远端戳后，新戳必须因果更晚");
    assert_eq!(tb.physical_ms, 1000);
    // 反向同样成立
    a.observe(&tb);
    let ta2 = a.tick();
    assert!(ta2 > tb);
}

#[test]
fn concurrent_stamps_break_ties_by_writer() {
    let mut a = Clock::with_now(1, fixed(1000));
    let mut b = Clock::with_now(2, fixed(1000));
    let ta = a.tick(); // (1000, 0, 1)
    let tb = b.tick(); // (1000, 0, 2)
    assert_ne!(ta, tb);
    assert!(tb > ta, "并发戳按写者 id 破平");
}

#[test]
fn display_parse_roundtrip() {
    for t in [Hlc::ZERO, Hlc::new(123, 4, 5)] {
        assert_eq!(t.to_string().parse::<Hlc>(), Ok(t));
    }
    assert!("x:0:0".parse::<Hlc>().is_err());
    assert!("1:2".parse::<Hlc>().is_err());
}

#[test]
fn hostile_far_future_stamp_does_not_freeze_the_clock() {
    // H3：远端戳 = (物理 u64::MAX, 计数 u32::MAX) 不能把本地时钟冻死
    //（旧实现：之后每次 tick 都返回同一戳 ⇒ 合并时被去重丢掉、本地写入静默消失）。
    let mut c = Clock::with_now(1, fixed(1000));
    c.observe(&Hlc::new(u64::MAX, u32::MAX, 9));
    let a = c.tick();
    let b = c.tick();
    let d = c.tick();
    assert!(b > a && d > b, "冻死的时钟：{a:?} {b:?} {d:?}");
    // 物理位被夹到本地 + 容差附近，而不是停留在 u64::MAX。
    assert!(c.last().physical_ms < u64::MAX);
}

#[test]
fn counter_overflow_carries_into_physical() {
    // 计数到顶：succeed 应进位到物理位，而不是饱和成同一戳。
    let mut c = Clock::with_now(1, fixed(1000));
    c.observe(&Hlc::new(1000, u32::MAX, 2));
    let t1 = c.tick();
    let t2 = c.tick();
    assert!(t2 > t1, "计数溢出未进位：{t1:?} {t2:?}");
}

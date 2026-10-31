//! 混合逻辑时钟（Hybrid Logical Clock, Kulkarni et al. 2014）。
//!
//! 戳 = `(物理毫秒, 逻辑计数, 写者 id)`：
//!
//! - **单调**：同写者严格递增；物理钟停走/回拨只推计数，不回退物理位；
//! - **因果**：[`Clock::observe`] 远端戳后，本地新戳在全序上必然更晚；
//! - **全序**：`(物理, 计数, 写者)` 字典序——并发戳按写者破平，多副本可确定性归并
//!   （见 [`merge_order`](crate::merge_order)，跨副本 Delta 同步的前置原语）。
//!
//! 物理源可注入（[`Clock::with_now`]），测试与回放完全确定。

use std::fmt;
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

/// 一个 HLC 时间戳。字典序 = 因果全序（同写者内严格递增）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Hlc {
    pub physical_ms: u64,
    pub counter: u32,
    pub writer: u32,
}

impl Hlc {
    pub const ZERO: Hlc = Hlc {
        physical_ms: 0,
        counter: 0,
        writer: 0,
    };

    pub fn new(physical_ms: u64, counter: u32, writer: u32) -> Self {
        Self {
            physical_ms,
            counter,
            writer,
        }
    }

    /// 真实物理时钟（Unix 毫秒）；系统时钟异常时回退 0。
    pub fn physical_now() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
}

impl fmt::Display for Hlc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}:{}", self.physical_ms, self.counter, self.writer)
    }
}

impl FromStr for Hlc {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bad = || format!("非法 HLC 戳 {s:?}（应为 物理:计数:写者）");
        let mut f = s.splitn(3, ':');
        let physical_ms = f
            .next()
            .ok_or_else(bad)?
            .parse::<u64>()
            .map_err(|_| bad())?;
        let counter = f
            .next()
            .ok_or_else(bad)?
            .parse::<u32>()
            .map_err(|_| bad())?;
        let writer = f
            .next()
            .ok_or_else(bad)?
            .parse::<u32>()
            .map_err(|_| bad())?;
        Ok(Self {
            physical_ms,
            counter,
            writer,
        })
    }
}

/// 混合逻辑时钟：单写者视角的 `last` 戳 + 可注入物理源。
pub struct Clock {
    writer: u32,
    last: Hlc,
    now_fn: Box<dyn Fn() -> u64 + Send + Sync>,
}

impl fmt::Debug for Clock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Clock")
            .field("writer", &self.writer)
            .field("last", &self.last)
            .finish()
    }
}

impl Clock {
    /// 真实物理时钟源。
    pub fn new(writer: u32) -> Self {
        Self::with_now(writer, Box::new(Hlc::physical_now))
    }

    /// 注入物理时钟源（测试/回放用）。
    pub fn with_now(writer: u32, now_fn: Box<dyn Fn() -> u64 + Send + Sync>) -> Self {
        Self {
            writer,
            last: Hlc::ZERO,
            now_fn,
        }
    }

    /// 本地事件：发一个新戳。
    pub fn tick(&mut self) -> Hlc {
        let pt = (self.now_fn)();
        self.last = if pt > self.last.physical_ms {
            Hlc {
                physical_ms: pt,
                counter: 0,
                writer: self.writer,
            }
        } else {
            // 物理钟停走/回拨：保持物理位，只推计数
            Hlc {
                physical_ms: self.last.physical_ms,
                counter: self.last.counter.saturating_add(1),
                writer: self.writer,
            }
        };
        self.last
    }

    /// 收到远端戳：并入本地状态，保证后续 [`Clock::tick`] 因果上更晚。
    pub fn observe(&mut self, remote: &Hlc) {
        let pt = (self.now_fn)();
        let own = self.last;
        let l = own.physical_ms.max(remote.physical_ms).max(pt);
        let c = if l == own.physical_ms && l == remote.physical_ms {
            own.counter.max(remote.counter).saturating_add(1)
        } else if l == own.physical_ms {
            own.counter.saturating_add(1)
        } else if l == remote.physical_ms {
            remote.counter.saturating_add(1)
        } else {
            0 // 物理钟跳到未来，重新起算
        };
        self.last = Hlc {
            physical_ms: l,
            counter: c,
            writer: self.writer,
        };
    }

    pub fn last(&self) -> Hlc {
        self.last
    }

    pub fn writer(&self) -> u32 {
        self.writer
    }
}

#[cfg(test)]
mod tests {
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
}

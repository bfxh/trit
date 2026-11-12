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

/// 远端物理位允许领先本地的时间容差：超出即视为异常戳（时钟跑飞/伪造），夹到
/// `本地 + 容差`——避免一个 `u64::MAX` 戳把本地时钟永久冻住、之后本地写入被去重丢光（H3）。
const MAX_SKEW_MS: u64 = 3_600_000;

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

    /// 下一个戳：计数 +1；溢出则物理位 +1ms、计数归零——保证**严格递增**，
    /// 不因 `u32::MAX` 饱和而冻结（H3 丢数据的根因）。
    fn succeed(physical_ms: u64, counter: u32, writer: u32) -> Hlc {
        match counter.checked_add(1) {
            Some(c) => Hlc {
                physical_ms,
                counter: c,
                writer,
            },
            None => Hlc {
                physical_ms: physical_ms.saturating_add(1),
                counter: 0,
                writer,
            },
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
            // 物理钟停走/回拨：保持物理位，推计数（溢出进位到物理位，绝不冻死）
            Hlc::succeed(self.last.physical_ms, self.last.counter, self.writer)
        };
        self.last
    }

    /// 收到远端戳：并入本地状态，保证后续 [`Clock::tick`] 因果上更晚。
    pub fn observe(&mut self, remote: &Hlc) {
        let pt = (self.now_fn)();
        let own = self.last;
        // 远端物理位超出本地+容差 ⇒ 异常戳：夹住（否则 u64::MAX 会冻死时钟，H3）。
        let remote_phys = remote.physical_ms.min(pt.saturating_add(MAX_SKEW_MS));
        let l = own.physical_ms.max(remote_phys).max(pt);
        self.last = if l == own.physical_ms && l == remote_phys {
            Hlc::succeed(l, own.counter.max(remote.counter), self.writer)
        } else if l == own.physical_ms {
            Hlc::succeed(l, own.counter, self.writer)
        } else if l == remote_phys {
            Hlc::succeed(l, remote.counter, self.writer)
        } else {
            // 物理钟跳到未来，重新起算
            Hlc {
                physical_ms: l,
                counter: 0,
                writer: self.writer,
            }
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
mod tests;

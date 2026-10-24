//! 双轨调度策略（纯函数 + 单元测试）。
//!
//! 规则来自用户 2026-10-25 的取舍：**三进制轨优先**（自己造的构建工具先跑），
//! 二进制轨（cargo/rustc/LLVM）只吃**剩余槽位**；也可以把优先给二进制轨（`--prefer binary`），
//! 或按提交顺序（`--prefer balanced`，由执行器用单一 FIFO 实现，不进本模块）。
//!
//! 这里只做"下一份工作该给谁"的判定，不碰线程、不碰 I/O——这样策略能被穷举单测。

/// 两条构建轨。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Track {
    /// 三进制轨：本工具的 `.tri → TRB1` 任务。
    Ternary,
    /// 二进制轨：外部 `cargo`/`rustc`/`llvm` 命令。
    Binary,
}

/// 优先支持哪一轨。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prefer {
    /// 三进制优先，二进制吃剩余槽位（默认）。
    Ternary,
    /// 二进制优先，三进制吃剩余槽位。
    Binary,
    /// 按清单顺序（执行器直接 FIFO，不经过本模块）。
    Balanced,
}

/// 调度器看到的负载快照。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Load {
    pub ternary_pending: usize,
    pub binary_pending: usize,
    pub ternary_running: usize,
    pub binary_running: usize,
}

impl Load {
    fn running(&self) -> usize {
        self.ternary_running + self.binary_running
    }
}

/// 下一份工作给哪条轨；返回 `None` 表示此刻没有可派发的工作。
pub fn pick(load: Load, prefer: Prefer, slots: usize) -> Option<Track> {
    if slots == 0 || load.running() >= slots {
        return None;
    }
    let (primary, secondary, primary_pending, secondary_pending, primary_running) = match prefer {
        // Balanced 在调用方处理；兜底行为 = 三进制优先，避免误派。
        Prefer::Ternary | Prefer::Balanced => (
            Track::Ternary,
            Track::Binary,
            load.ternary_pending,
            load.binary_pending,
            load.ternary_running,
        ),
        Prefer::Binary => (
            Track::Binary,
            Track::Ternary,
            load.binary_pending,
            load.ternary_pending,
            load.binary_running,
        ),
    };
    if primary_pending > 0 && primary_running < slots {
        return Some(primary);
    }
    if secondary_pending > 0 {
        return Some(secondary);
    }
    if primary_pending > 0 {
        return Some(primary);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load(t_pending: usize, b_pending: usize, t_running: usize, b_running: usize) -> Load {
        Load {
            ternary_pending: t_pending,
            binary_pending: b_pending,
            ternary_running: t_running,
            binary_running: b_running,
        }
    }

    #[test]
    fn ternary_wins_while_slots_remain() {
        assert_eq!(
            pick(load(3, 5, 0, 0), Prefer::Ternary, 2),
            Some(Track::Ternary)
        );
        assert_eq!(
            pick(load(3, 5, 1, 0), Prefer::Ternary, 2),
            Some(Track::Ternary)
        );
    }

    #[test]
    fn binary_takes_leftover_slots_only_after_ternary_saturates() {
        // 三进制已排空 ⇒ 二进制吃满空出来的槽位。
        assert_eq!(
            pick(load(0, 5, 0, 0), Prefer::Ternary, 2),
            Some(Track::Binary)
        );
        assert_eq!(
            pick(load(0, 5, 0, 1), Prefer::Ternary, 2),
            Some(Track::Binary)
        );
        // 槽位已满 ⇒ 谁都不派（不抢占）。
        assert_eq!(pick(load(3, 5, 2, 0), Prefer::Ternary, 2), None);
        // 三进制还有待办、槽位没满 ⇒ 绝不派二进制。
        assert_eq!(
            pick(load(1, 1, 0, 1), Prefer::Ternary, 2),
            Some(Track::Ternary)
        );
    }

    #[test]
    fn binary_preference_mirrors_the_policy() {
        assert_eq!(
            pick(load(5, 2, 0, 0), Prefer::Binary, 2),
            Some(Track::Binary)
        );
        // 二进制已占满槽位 ⇒ 三进制吃剩余（此处无名额，返回 None）。
        assert_eq!(pick(load(5, 2, 0, 2), Prefer::Binary, 2), None);
        // 二进制仍有待办但槽位未满 ⇒ 继续二进制。
        assert_eq!(
            pick(load(5, 2, 0, 1), Prefer::Binary, 2),
            Some(Track::Binary)
        );
    }

    #[test]
    fn never_exceeds_slots_or_returns_none_without_work() {
        assert_eq!(pick(load(9, 9, 2, 1), Prefer::Ternary, 3), None);
        assert_eq!(pick(load(0, 0, 0, 0), Prefer::Ternary, 4), None);
        assert_eq!(pick(load(1, 0, 0, 0), Prefer::Ternary, 0), None);
    }
}

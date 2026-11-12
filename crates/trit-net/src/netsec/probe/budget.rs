//! 探测资源预算的纯谓词（从 probe.rs 拆出，纯搬家不改语义）：总探测数门 + 限速睡眠预算。
//!
//! 拆出来是为了给它们**确定性边界单元测试**：旧内联实现只被一个依赖真实网络计时的集成测试
//! 覆盖——快机器上探测耗时不超过 deadline，`deadline - elapsed` 不下溢，下溢减法变异就杀不掉
//! （CI ubuntu 存活、本机 Windows 才抓住）。纯谓词单测直接喂 elapsed>deadline，任何平台都确定性抓住。

use std::time::Duration;

use super::HARD_MAX_TOTAL_PROBES;

/// 总探测数门（纯函数，不分配）：去重后的端口总数在硬顶内才放行。
/// 目标数与每目标端口数各自合规，聚合仍可能是扫描（64×32=2048）——这道门补上聚合量纲。
pub(super) fn total_probes_ok(total: usize) -> bool {
    total <= HARD_MAX_TOTAL_PROBES
}

/// 限速睡眠预算（纯函数）：只睡「间隔」与「deadline 剩余」的较小者，绝不睡过 deadline。
/// 旧实现在每次探测后无条件 `sleep(delay)`，delay 顶到 60s 时能把全局 deadline 大幅 overshoot。
pub(super) fn sleep_budget(delay: Duration, elapsed: Duration, deadline: Duration) -> Duration {
    let remaining = deadline.saturating_sub(elapsed);
    delay.min(remaining)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn total_probes_ok_boundary() {
        assert!(total_probes_ok(0));
        // 恰好到顶 ⇒ 放行；超一个 ⇒ 拒绝（边界写成严格小于会在这里红）。
        assert!(total_probes_ok(HARD_MAX_TOTAL_PROBES));
        // saturating_add 而非 +1：死帽变异（帽=usize::MAX）下 `MAX+1` 常量溢出编译红=假杀。
        assert!(!total_probes_ok(HARD_MAX_TOTAL_PROBES.saturating_add(1)));
    }

    #[test]
    fn sleep_budget_clamps_to_zero_when_elapsed_exceeds_deadline() {
        // elapsed > deadline：剩余为 0，绝不 panic。下溢减法变异（deadline - elapsed）会在这里炸。
        assert_eq!(
            sleep_budget(
                Duration::from_secs(3),
                Duration::from_millis(300),
                Duration::from_millis(200)
            ),
            Duration::ZERO
        );
    }

    #[test]
    fn sleep_budget_boundary_elapsed_equals_deadline() {
        let d = Duration::from_millis(200);
        assert_eq!(sleep_budget(Duration::from_secs(3), d, d), Duration::ZERO);
    }

    #[test]
    fn sleep_budget_takes_remaining_when_less_than_delay() {
        // 剩余 50ms < delay 3s：夹到剩余，不睡满 delay（不夹剩余的变异会在这里红）。
        assert_eq!(
            sleep_budget(
                Duration::from_secs(3),
                Duration::from_millis(150),
                Duration::from_millis(200)
            ),
            Duration::from_millis(50)
        );
    }

    #[test]
    fn sleep_budget_takes_delay_when_remaining_is_ample() {
        // 剩余 190ms > delay 50ms：睡满 delay。
        assert_eq!(
            sleep_budget(
                Duration::from_millis(50),
                Duration::from_millis(10),
                Duration::from_millis(200)
            ),
            Duration::from_millis(50)
        );
    }
}

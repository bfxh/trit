//! `Journal::adopt` 的校验前置（第三轮 R1/R2；拆出 `journal.rs` 守模块行数）。
//!
//! 先校验、后落盘：任何一条不合法都不动现有日志；未来戳直接拒绝——LWW 全序里
//! `physical_ms` 是第一关键字，`u64::MAX` 会永久压制该节点的真实裁决。

use std::io;

use crate::dag::ImmuneDag;
use crate::delta::Stamped;
use crate::hlc::{self, Clock};

/// 校验整份 canonical 并构造对应的 DAG/时钟；失败时没有任何副作用。
pub(super) fn prepare(canonical: &[Stamped], writer: u32) -> io::Result<(ImmuneDag, Clock)> {
    let ceiling = hlc::Hlc::physical_now().saturating_add(hlc::MAX_SKEW_MS);
    if let Some(s) = canonical.iter().find(|s| s.ts.physical_ms > ceiling) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "戳 {} 超出本地时钟容差（{} > {ceiling}），拒绝采纳",
                s.ts, s.ts.physical_ms
            ),
        ));
    }
    let mut dag = ImmuneDag::empty();
    let mut clock = Clock::new(writer);
    for s in canonical {
        clock.observe(&s.ts);
        dag.apply(&s.delta)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    }
    Ok((dag, clock))
}

//! `checkout` 时间旅行视图构造（从 bin/trit.rs 拆出，控制单文件行数预算）。

use super::*;
use trit_immune::ImmuneDag;

/// 构造 `tip` 节点时刻的只读视图：与账本前缀 **1:1 对齐**——编号、父子、标题、
/// 裁决与历史逐条带回。
///
/// 旧实现先 `ImmuneDag::root("时间旅行视图")` 建合成根，再把账本原根作为新节点
/// 重放：视图恒比正确结果多 1 个节点，原根被错挂成合成根的子节点，子节点编号
/// 整体后移（审计 F39-1 / #619）。这里从空 DAG 起步：账本原根直接落到视图 #0，
/// 其余节点按账本顺序以 1:1 编号挂回父节点。
pub(super) fn build_checkout_view(dag: &ImmuneDag, tip: u32) -> Result<ImmuneDag, CliError> {
    let mut view = ImmuneDag::empty();
    for (i, node) in dag.nodes()[..=tip as usize].iter().enumerate() {
        let id = NodeId(i as u32);
        if node.parent.is_none() {
            // 账本根：直接落到空视图 #0；基线 +1 由 Root 重放恢复，再带回基线之后的历史。
            view.apply(&delta::Delta::Root {
                title: node.title.clone(),
            })
            .map_err(|e| CliError::Runtime(e.to_string()))?;
            for e in node.history.iter().skip(1) {
                view.record(id, e.verdict, None, &e.note);
            }
        } else {
            let parent = node.parent.expect("非根节点必有父节点");
            let parent = NodeId(parent.0.min(i.saturating_sub(1) as u32));
            view.spawn(parent, node.kind, &node.title);
            if node.quarantined {
                view.quarantine(id, "checkout：保留隔离标记");
            }
            for e in &node.history {
                view.record(id, e.verdict, None, &e.note);
            }
        }
    }
    Ok(view)
}

#[cfg(test)]
#[path = "checkout_tests.rs"]
mod tests;

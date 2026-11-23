//! `checkout` 时间旅行视图构造（从 bin/trit.rs 拆出，控制单文件行数预算）。

use super::*;
use trit_immune::ImmuneDag;

/// 构造 `tip` 节点时刻的只读视图：与账本前缀 **1:1 对齐**——编号、父子、标题、
/// 裁决与历史逐条带回。
///
/// 旧实现先建合成根再重放原根，视图恒多 1 个节点（F39-1 / #619）；这里从空 DAG 起步，原根直接落到 #0，其余节点按账本顺序 1:1 挂回父节点。
pub(super) fn build_checkout_view(dag: &ImmuneDag, tip: u32) -> Result<ImmuneDag, CliError> {
    let mut view = ImmuneDag::empty();
    for (i, node) in dag.nodes()[..=tip as usize].iter().enumerate() {
        let id = NodeId(i as u32);
        match node.parent {
            None => {
                // 账本根：直接落到空视图 #0；基线 +1 由 Root 重放恢复，再带回基线之后的历史。
                let root = delta::Delta::Root {
                    title: node.title.clone(),
                };
                view.apply(&root)
                    .map_err(|e| CliError::Runtime(e.to_string()))?;
                for e in node.history.iter().skip(1) {
                    view.record(id, e.verdict, None, &e.note);
                }
            }
            Some(parent) => {
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
    }
    Ok(view)
}

#[cfg(test)]
#[path = "checkout_tests.rs"]
mod tests;

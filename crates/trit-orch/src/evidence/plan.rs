use serde::{Deserialize, Serialize};

/// 回执钉住的计划身份：快照 revision + 任务定义摘要。
///
/// 只有两项都匹配，完成闸才承认这条回执；它防止「同一任务 ID 在不同计划
/// revision 下换了定义」时旧回执继续生效。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptPlan {
    pub revision: u64,
    pub task_digest: String,
}

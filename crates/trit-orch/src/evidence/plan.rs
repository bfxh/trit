use serde::{Deserialize, Serialize};

/// 回执钉住的计划身份：快照 revision + 验收身份摘要。
///
/// `acceptance_digest` 覆盖任务定义与本次采用的最低断言数；防止「同一任务
/// ID 在不同 revision 下换了定义」或「测量后换阈值」时旧回执继续生效。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptPlan {
    pub revision: u64,
    pub acceptance_digest: String,
}

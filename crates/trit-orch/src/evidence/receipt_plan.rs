use super::{Receipt, ReceiptPlan};

impl Receipt {
    /// 绑定计划身份：快照 revision + 验收身份摘要。
    ///
    /// 宿主测量时用 `acceptance_digest(task, min_assertions)` 得到摘要；默认
    /// 完成闸要求它和当前快照/任务定义/阈值逐项相等，防止换阈值复用回执。
    pub fn with_plan(mut self, revision: u64, acceptance_digest: impl Into<String>) -> Self {
        self.plan = Some(ReceiptPlan {
            revision,
            acceptance_digest: acceptance_digest.into(),
        });
        self
    }
}

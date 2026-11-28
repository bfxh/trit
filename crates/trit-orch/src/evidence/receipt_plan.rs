use super::{Receipt, ReceiptPlan};

impl Receipt {
    /// 绑定计划身份：快照 revision + 任务定义摘要。
    ///
    /// 宿主在测量时用 `task_spec_digest` 得到摘要；默认完成闸会要求它和当前
    /// 快照/任务定义逐项相等，防止旧 revision 的同名任务回执被复用。
    pub fn with_plan(mut self, revision: u64, task_digest: impl Into<String>) -> Self {
        self.plan = Some(ReceiptPlan {
            revision,
            task_digest: task_digest.into(),
        });
        self
    }
}

//! 合并报告（从 `task_merge.rs` 拆出，控制单文件行数）。

/// 合并报告：投票/平票/忽略/改动/scale 行数。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergeReport {
    pub(crate) tasks: usize,
    pub(crate) ignored_tasks: usize,
    pub(crate) voted: usize,
    pub(crate) ties: usize,
    pub(crate) changed: usize,
    pub(crate) scale_rows: usize,
    /// DARE 稀疏化丢弃的差分坐标数；普通合并为 0。
    pub(crate) dropped: usize,
}

impl MergeReport {
    /// 参与合并的任务数。
    #[must_use]
    pub fn tasks(&self) -> usize {
        self.tasks
    }

    /// 权重为 0、完全不参与的任务数。
    #[must_use]
    pub fn ignored_tasks(&self) -> usize {
        self.ignored_tasks
    }

    /// 至少收到一票的坐标数。
    #[must_use]
    pub fn voted(&self) -> usize {
        self.voted
    }

    /// 平票（票和为零且票值不全相同）而保留基准的坐标数。
    #[must_use]
    pub fn ties(&self) -> usize {
        self.ties
    }

    /// 实际写出的 trit/偏置编辑数。
    #[must_use]
    pub fn changed(&self) -> usize {
        self.changed
    }

    /// 实际写出的 scale 行数。
    #[must_use]
    pub fn scale_rows(&self) -> usize {
        self.scale_rows
    }

    /// DARE 稀疏化丢弃的差分坐标数；普通合并为 0。
    #[must_use]
    pub fn dropped(&self) -> usize {
        self.dropped
    }

    /// 合并报告的 JSON 形状（CLI/MCP 共用）。
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "tasks": self.tasks,
            "ignored_tasks": self.ignored_tasks,
            "voted": self.voted,
            "ties": self.ties,
            "changed": self.changed,
            "scale_rows": self.scale_rows,
            "dropped": self.dropped,
        })
    }

    /// 给 DARE 路径的投票报告补上实际丢弃数。
    pub(crate) fn with_dropped(mut self, dropped: usize) -> Self {
        self.dropped = dropped;
        self
    }
}

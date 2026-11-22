use crate::{Error, Snapshot, TaskSpec};

use crate::{Progress, Summary};

impl Progress {
    /// 完成闸门：任何未定/未验的任务都让「做完了」判红。
    ///
    /// 这是库层旧入口：只从快照计事实，不要求回执 provenance。宿主/CLI
    /// 应使用 [`Self::finish_with_receipts`] 把完成声明钉到受信 executor 回执上。
    #[deprecated(
        note = "snapshot-only completion trusts a bare CheckReport; use finish_with_receipts or explicitly allow legacy"
    )]
    pub fn finish(&self, tasks: &[TaskSpec], snapshot: &Snapshot) -> Result<Summary, Error> {
        self.finish_inner(tasks, snapshot, None)
    }
}

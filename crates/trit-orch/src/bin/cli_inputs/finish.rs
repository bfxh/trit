use std::collections::BTreeMap;

use serde::Deserialize;
use trit_orch::{Receipt, RoundReport, Snapshot, TaskSpec};

fn default_min_assertions() -> u32 {
    1
}

#[derive(Deserialize)]
pub(crate) struct FinishInput {
    pub(crate) tasks: Vec<TaskSpec>,
    pub(crate) snapshot: Snapshot,
    #[serde(default)]
    pub(crate) rounds: Vec<RoundReport>,
    /// 可信 executor 回执；默认 finish 要求每个 decisive task 都有一条。
    #[serde(default)]
    pub(crate) receipts: BTreeMap<String, Receipt>,
    /// 回执重算验收报告时的最低断言数；默认 1，零断言不会变成 Passed。
    #[serde(default = "default_min_assertions")]
    pub(crate) min_assertions: u32,
}

//! `BertConfig` 的构造器（守住行数预算）。
use super::{BertConfig, Budget};
impl BertConfig {
    /// `group == 0` 表示 per-row scale；`dir` 为空表示不做 GPTQ。
    pub(crate) fn with_gptq(mut self, dir: Option<String>, group: usize) -> Self {
        self.gptq_dir = dir;
        self.gptq_group = group;
        self
    }
    pub(crate) fn with_sidecar(mut self, rank: usize) -> Self {
        self.sidecar_rank = rank;
        self
    }

    pub(crate) fn with_sidecar_svd(mut self, rank: usize) -> Self {
        self.sidecar_svd = rank;
        self
    }
    pub(crate) fn with_budget(mut self, budget: Option<Budget>) -> Self {
        self.budget = budget;
        self
    }
}

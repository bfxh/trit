//! `BertConfig` 的构造器（从 `config.rs` 拆出守住行数预算）。

use super::BertConfig;

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

    /// 低秩残差侧车（`R = W − s·T` 的 top-r SVD）。
    pub(crate) fn with_sidecar_svd(mut self, rank: usize) -> Self {
        self.sidecar_svd = rank;
        self
    }
}

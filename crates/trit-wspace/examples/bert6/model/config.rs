//! `BertConfig` 与构造器（从 `model.rs` 拆出，守住行数预算）。
use crate::activation::ActivationMode;

pub(crate) struct BertConfig {
    pub(crate) layers: usize,
    pub(crate) hidden: usize,
    pub(crate) heads: usize,
    pub(crate) eps: f32,
    pub(crate) quantize: bool,
    pub(crate) activation: ActivationMode,
    pub(crate) sidecar_rank: usize,
    pub(crate) gptq_dir: Option<String>,
    pub(crate) gptq_group: usize,
}

impl BertConfig {
    /// MiniLM-L6 的固定形状：6 层、12 heads、FFN 4×hidden。
    pub(crate) fn new(hidden: usize, eps: f32, quantize: bool, activation: ActivationMode) -> Self {
        Self {
            layers: 6,
            hidden,
            heads: 12,
            eps,
            quantize,
            activation,
            sidecar_rank: 0,
            gptq_dir: None,
            gptq_group: 0,
        }
    }

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
}

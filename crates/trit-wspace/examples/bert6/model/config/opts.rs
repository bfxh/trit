//! `BertConfig` 定义（守住行数预算）。
pub(crate) use super::budget::Budget;
use crate::activation::ActivationMode;
mod env;
use env::{env_layers, env_u32, env_usize};

pub(crate) struct BertConfig {
    pub(crate) layers: usize,
    pub(crate) hidden: usize,
    pub(crate) heads: usize,
    pub(crate) eps: f32,
    pub(crate) quantize: bool,
    pub(crate) activation: ActivationMode,
    pub(crate) sidecar_rank: usize,
    pub(crate) sidecar_svd: usize,
    pub(crate) gptq_dir: Option<String>,
    pub(crate) gptq_group: usize,
    pub(crate) weight_bits: u32,
    pub(crate) uniform_gptq: bool,
    pub(crate) f32_layers: Vec<usize>,
    pub(crate) budget: Option<Budget>,
}

impl BertConfig {
    /// MiniLM-L6 的固定形状：6 层、12 heads、FFN 4×hidden。
    pub(crate) fn new(hidden: usize, eps: f32, quantize: bool, activation: ActivationMode) -> Self {
        let bits = env_u32("TRIT_WEIGHT_BITS", 2);
        assert!((2..=8).contains(&bits), "TRIT_WEIGHT_BITS 要在 2..=8");
        Self {
            layers: env_usize("TRIT_LAYERS", 6),
            hidden,
            heads: 12,
            eps,
            quantize,
            activation,
            sidecar_rank: 0,
            sidecar_svd: 0,
            gptq_dir: None,
            gptq_group: 0,
            weight_bits: bits,
            uniform_gptq: std::env::var("TRIT_UNIFORM_GPTQ")
                .ok()
                .is_some_and(|value| value.trim() == "1"),
            f32_layers: env_layers("TRIT_F32_LAYERS"),
            budget: None,
        }
    }
}

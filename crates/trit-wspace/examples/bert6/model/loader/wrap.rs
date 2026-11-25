//! 权重包装：三值 / 均匀位宽 / 逐层 f32 回退（从 `loader.rs` 拆出守住行数预算）。

use super::h;
use crate::linear::{F32Linear, Linear, QuantLinear};
use crate::model::BertConfig;
mod uniform;

pub(super) fn build(linear: F32Linear, layer: usize, short: &str, config: &BertConfig) -> Linear {
    if !config.quantize || config.f32_layers.contains(&layer) {
        return Linear::F32(linear);
    }
    if config.weight_bits >= 3 {
        return uniform::build(linear, short, config);
    }
    let h = h::load(config.gptq_dir.as_deref(), short);
    Linear::Quant(QuantLinear::from(
        &linear,
        config.sidecar_rank,
        config.sidecar_svd,
        h.as_deref(),
        config.gptq_group,
    ))
}

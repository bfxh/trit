use super::h;
use crate::linear::{F32Linear, Linear, QuantLinear};
use crate::model::BertConfig;
mod uniform;

pub(super) fn build(linear: F32Linear, layer: usize, short: &str, config: &BertConfig) -> Linear {
    if !config.quantize || config.f32_layers.contains(&layer) {
        return Linear::F32(linear);
    }
    let bits = config.budget.as_ref().and_then(|b| b.bits(short));
    let bits = bits.unwrap_or(config.weight_bits);
    if bits >= 3 {
        return uniform::build(linear, short, config, bits);
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

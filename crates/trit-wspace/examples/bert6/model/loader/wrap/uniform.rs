//! 均匀位宽分支：RTN 或可选 GPTQ，逐层 f32 回退已在 `wrap` 里先行处理。

use super::super::h;
use crate::linear::{F32Linear, Linear, uniform};
use crate::model::BertConfig;

pub(super) fn build(linear: F32Linear, short: &str, config: &BertConfig) -> Linear {
    assert!(
        config.sidecar_svd == 0,
        "均匀位宽暂不叠加 SVD 侧车（侧车只在三值残差上定义）"
    );
    let group = if config.gptq_group == 0 {
        32
    } else {
        config.gptq_group
    };
    if config.uniform_gptq
        && let Some(h) = h::load(config.gptq_dir.as_deref(), short)
    {
        return Linear::F32(
            uniform::dequantize_gptq(&linear, config.weight_bits, group, &h)
                .expect("GPTQ 均匀位宽量化失败"),
        );
    }
    Linear::F32(uniform::dequantize(&linear, config.weight_bits, group))
}

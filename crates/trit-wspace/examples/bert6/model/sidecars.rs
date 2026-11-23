//! `Bert` 的 SVD 侧车 rank 控制与参数量统计（从 `model.rs` 拆出守住行数预算）。

use super::{Bert, BertConfig};

impl Bert {
    pub(crate) fn load(file: &str, config: BertConfig) -> Self {
        super::loader::load(file, config)
    }

    /// 收缩所有线性层的 SVD 侧车 rank（前缀截断，读数关于 rank 单调）。
    pub(crate) fn set_sidecar_rank(&mut self, rank: usize) {
        for layer in &mut self.layers {
            layer.query.set_sidecar_rank(rank);
            layer.key.set_sidecar_rank(rank);
            layer.value.set_sidecar_rank(rank);
            layer.output.set_sidecar_rank(rank);
            layer.intermediate.set_sidecar_rank(rank);
            layer.ffn_output.set_sidecar_rank(rank);
        }
    }

    /// 全部侧车的 f32 参数个数。
    pub(crate) fn sidecar_params(&self) -> usize {
        self.layers
            .iter()
            .map(|layer| {
                layer.query.sidecar_params()
                    + layer.key.sidecar_params()
                    + layer.value.sidecar_params()
                    + layer.output.sidecar_params()
                    + layer.intermediate.sidecar_params()
                    + layer.ffn_output.sidecar_params()
            })
            .sum()
    }

    /// 全部线性层 f32 权重的参数个数（三值主干同位置）。
    pub(crate) fn weight_params(&self) -> usize {
        self.layers
            .iter()
            .map(|layer| {
                layer.query.weight_params()
                    + layer.key.weight_params()
                    + layer.value.weight_params()
                    + layer.output.weight_params()
                    + layer.intermediate.weight_params()
                    + layer.ffn_output.weight_params()
            })
            .sum()
    }
}

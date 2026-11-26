//! `Linear` 的侧车与参数量访问器（从 `linear.rs` 拆出守住行数预算）。

use super::Linear;

impl Linear {
    pub(crate) fn set_sidecar_rank(&mut self, rank: usize) {
        if let Self::Quant(linear) = self {
            linear.set_sidecar_rank(rank);
        }
    }

    pub(crate) fn sidecar_params(&self) -> usize {
        match self {
            Self::F32(_) => 0,
            Self::Quant(linear) => linear.sidecar_params(),
        }
    }

    pub(crate) fn weight_params(&self) -> usize {
        match self {
            Self::F32(linear) => linear.rows * linear.cols,
            Self::Quant(linear) => linear.weight_params(),
        }
    }
}

//! matmul 输入量化：Mixed 用三值，Int8 用 per-token int8。

use crate::activation::{ActivationMode, quantize_activation, quantize_int8};

pub(crate) fn quantize(
    values: &[f32],
    width: usize,
    activation: ActivationMode,
    scales: Option<&[f32]>,
) -> Vec<f32> {
    if !activation.matmul_input() {
        return values.to_vec();
    }
    let mut copy = values.to_vec();
    if activation == ActivationMode::Int8 {
        quantize_int8(&mut copy, width);
    } else {
        quantize_activation(&mut copy, ActivationMode::TernaryAllChannel, scales);
    }
    copy
}

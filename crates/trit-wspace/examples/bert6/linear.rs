//! 线性层：f32 / 三值（absmean 或 GPTQ 逐列补偿，per-row 或 per-group scale + f32 bias）。
mod dense;
mod kinds;
mod quant;
pub(crate) use quant::QuantLinear;
pub(crate) struct F32Linear {
    pub(crate) rows: usize,
    pub(crate) cols: usize,
    pub(crate) weight: Vec<f32>,
    pub(crate) bias: Vec<f32>,
}
pub(crate) enum Linear {
    F32(F32Linear),
    Quant(QuantLinear),
}
impl Linear {
    pub(crate) fn forward_seq(&self, input: &[f32], seq: usize, out: &mut [f32]) {
        match self {
            Self::F32(linear) => linear.forward_seq(input, seq, out),
            Self::Quant(linear) => linear.forward_seq(input, seq, out),
        }
    }
}

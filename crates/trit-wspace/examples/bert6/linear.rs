//! 线性层：f32 / 三值（absmean 或 GPTQ 逐列补偿，per-row 或 per-group scale + f32 bias）。
mod quant;
pub(crate) use quant::QuantLinear;
pub(crate) struct F32Linear {
    pub(crate) rows: usize,
    pub(crate) cols: usize,
    pub(crate) weight: Vec<f32>,
    pub(crate) bias: Vec<f32>,
}
impl F32Linear {
    fn forward(&self, input: &[f32], out: &mut [f32]) {
        for (row, slot) in out.iter_mut().enumerate().take(self.rows) {
            let mut acc = f64::from(self.bias[row]);
            let line = &self.weight[row * self.cols..(row + 1) * self.cols];
            for (&weight, &value) in line.iter().zip(input) {
                acc += f64::from(weight) * f64::from(value);
            }
            *slot = acc as f32;
        }
    }
    pub(crate) fn forward_seq(&self, input: &[f32], seq: usize, out: &mut [f32]) {
        for token in 0..seq {
            self.forward(
                &input[token * self.cols..(token + 1) * self.cols],
                &mut out[token * self.rows..(token + 1) * self.rows],
            );
        }
    }
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

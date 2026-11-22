//! 线性层：f32 / 三值（absmean per-row scale + f32 bias）。

use crate::sidecar_layer::Sidecar;
use trit_wspace::safetensors::Weights;
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
pub(crate) struct QuantLinear {
    rows: usize,
    cols: usize,
    trits: Vec<i8>,
    scales: Vec<f32>,
    bias: Vec<f32>,
    sidecar: Option<Sidecar>,
}
impl QuantLinear {
    pub(crate) fn from(linear: &F32Linear, rank: usize) -> Self {
        let weights = Weights {
            rows: linear.rows,
            cols: linear.cols,
            values: linear.weight.clone(),
        };
        let layer = weights
            .to_layer_with_bias(Some(&linear.bias))
            .expect("三值化失败");
        let mut trits = Vec::with_capacity(linear.rows * linear.cols);
        let mut scales = Vec::with_capacity(linear.rows);
        for row in 0..linear.rows {
            scales.push(layer.scale(row).expect("scale"));
            for col in 0..linear.cols {
                trits.push(layer.trit(row, col).expect("trit").to_i8());
            }
        }
        let sidecar = Sidecar::build(linear, &trits, &scales, rank);
        Self {
            rows: linear.rows,
            cols: linear.cols,
            trits,
            scales,
            bias: linear.bias.clone(),
            sidecar,
        }
    }
    fn forward(&self, input: &[f32], out: &mut [f32]) {
        for (row, slot) in out.iter_mut().enumerate().take(self.rows) {
            let mut acc = 0.0f64;
            let line = &self.trits[row * self.cols..(row + 1) * self.cols];
            for (&trit, &value) in line.iter().zip(input) {
                acc += f64::from(trit) * f64::from(value);
            }
            let correction = self
                .sidecar
                .as_ref()
                .map_or(0.0, |sidecar| sidecar.correction(row, input));
            *slot =
                (f64::from(self.scales[row]) * acc + f64::from(self.bias[row]) + correction) as f32;
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

//! 三值线性层：absmean / GPTQ，per-row 或 per-group scale + f32 bias。
use super::F32Linear;
use crate::sidecar_layer::Sidecar;

pub(crate) struct QuantLinear {
    rows: usize,
    cols: usize,
    group: usize,
    groups: usize,
    trits: Vec<i8>,
    scales: Vec<f32>,
    bias: Vec<f32>,
    sidecar: Option<Sidecar>,
}

impl QuantLinear {
    /// `group == 0` 或 `group >= cols` 时退化为 per-row scale（每行一个）。
    /// `Sidecar` 只在 per-row 时使用（`run.rs` 的侧车扫描不开 GPTQ）。
    pub(crate) fn from(linear: &F32Linear, rank: usize, h: Option<&[f32]>, group: usize) -> Self {
        let group = if group == 0 {
            linear.cols
        } else {
            group.clamp(1, linear.cols)
        };
        let (trits, scales) = match h {
            Some(h) if h.len() == linear.cols * linear.cols => trit_wspace::gptq::quantize_grouped(
                &linear.weight,
                linear.rows,
                linear.cols,
                h,
                group,
            )
            .expect("GPTQ 三值化失败"),
            _ => trit_wspace::gptq::absmean(&linear.weight, linear.rows, linear.cols)
                .expect("absmean 三值化失败"),
        };
        let sidecar = Sidecar::build(linear, &trits, &scales, rank);
        Self {
            rows: linear.rows,
            cols: linear.cols,
            group,
            groups: linear.cols.div_ceil(group),
            trits,
            scales,
            bias: linear.bias.clone(),
            sidecar,
        }
    }

    fn forward(&self, input: &[f32], out: &mut [f32]) {
        for (row, slot) in out.iter_mut().enumerate().take(self.rows) {
            let base = row * self.cols;
            let mut total = 0.0f64;
            for chunk in 0..self.groups {
                let start = chunk * self.group;
                let end = (start + self.group).min(self.cols);
                let line = &self.trits[base + start..base + end];
                let mut acc = 0.0f64;
                for (&trit, &value) in line.iter().zip(&input[start..end]) {
                    acc += f64::from(trit) * f64::from(value);
                }
                total += f64::from(self.scales[row * self.groups + chunk]) * acc;
            }
            let correction = self
                .sidecar
                .as_ref()
                .map_or(0.0, |sidecar| sidecar.correction(row, input));
            *slot = (total + f64::from(self.bias[row]) + correction) as f32;
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

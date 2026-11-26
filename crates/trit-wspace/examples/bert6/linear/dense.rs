//! `F32Linear` 的前向（从 `linear.rs` 拆出守住行数预算）。

use super::F32Linear;

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

//! top-k 列 f32 侧车：从 `R = W - s·T` 里按列范数保留 k 列。
//! 从 `sidecar_layer.rs` 拆出，原文件只留两行接线（守住模块预算）。

use crate::linear::F32Linear;

pub(crate) struct Sidecar {
    cols: Vec<usize>,
    values: Vec<f32>,
}

impl Sidecar {
    pub(crate) fn build(
        linear: &F32Linear,
        trits: &[i8],
        scales: &[f32],
        rank: usize,
    ) -> Option<Self> {
        if rank == 0 {
            return None;
        }
        let mut residual = linear.weight.clone();
        for row in 0..linear.rows {
            let scale = scales[row];
            for col in 0..linear.cols {
                residual[row * linear.cols + col] -=
                    f32::from(trits[row * linear.cols + col]) * scale;
            }
        }
        let mut norms: Vec<(usize, f32)> = (0..linear.cols)
            .map(|col| {
                let norm = (0..linear.rows)
                    .map(|row| residual[row * linear.cols + col].abs())
                    .sum::<f32>();
                (col, norm)
            })
            .collect();
        norms.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let cols: Vec<usize> = norms
            .into_iter()
            .take(rank.min(linear.cols))
            .map(|(col, _)| col)
            .collect();
        let mut values = Vec::with_capacity(linear.rows * cols.len());
        for row in 0..linear.rows {
            for &col in &cols {
                values.push(residual[row * linear.cols + col]);
            }
        }
        Some(Self { cols, values })
    }

    pub(crate) fn correction(&self, row: usize, input: &[f32]) -> f64 {
        let width = self.cols.len();
        self.cols
            .iter()
            .enumerate()
            .map(|(index, &col)| {
                f64::from(self.values[row * width + index]) * f64::from(input[col])
            })
            .sum()
    }

    /// 列侧车占用的 f32 个数。
    pub(crate) fn params(&self) -> usize {
        self.values.len()
    }
}

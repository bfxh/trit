//! [`GroupedLayer`] 的运算：等效权重、位宽、前向。
//!
//! 与 [`crate::grouped`] 分开放，是因为"构造"和"使用"各自已经够长；拆开后两个文件都在
//! 模块台账的单文件上限之内。

use crate::error::{Error, Result};
use crate::grouped::GroupedLayer;

impl GroupedLayer {
    /// `(row, col)` 处的等效 f32 权重：`trit × scale[group]`。
    #[must_use]
    pub fn effective(&self, row: usize, col: usize) -> Option<f32> {
        if row >= self.rows() || col >= self.cols() {
            return None;
        }
        let trit = self.trit(row, col)?;
        let group = self.group_of(row, col)?;
        let scale = self.scale(group as usize)?;
        Some(f32::from(trit.to_i8()) * scale)
    }

    /// 每权重实际位宽：`2`（trit）+ `32/G`（scale）+ `log2(组数)`（组号）。
    ///
    /// 组号必须计入——漏掉它会低估 5 倍以上（本项目踩过这个坑，故有专门测试守着）。
    #[must_use]
    pub fn bits_per_weight(&self) -> f32 {
        let groups = self.group_count();
        let group_bits = if groups <= 1 {
            0.0
        } else {
            (groups as f32).log2()
        };
        2.0 + 32.0 / self.group_size() as f32 + group_bits
    }

    /// 等效 f32 权重矩阵（行主序）。
    #[must_use]
    pub fn to_f32(&self) -> Vec<f32> {
        let (rows, cols) = (self.rows(), self.cols());
        (0..rows * cols)
            .map(|index| {
                let row = index / cols;
                let col = index % cols;
                self.effective(row, col).unwrap_or(0.0)
            })
            .collect()
    }

    /// 一位前向：`y[r] = Σ_c trit×scale · x[c] + bias[r]`。
    pub fn forward(&self, input: &[f32]) -> Result<Vec<f32>> {
        let cols = self.cols();
        if input.len() != cols {
            return Err(Error::Shape {
                what: "grouped input",
                expected: cols,
                got: input.len(),
            });
        }
        let weights = self.to_f32();
        let mut output = self.bias_f32.clone();
        for (row, slot) in output.iter_mut().enumerate() {
            let line = &weights[row * cols..(row + 1) * cols];
            *slot += line.iter().zip(input).map(|(w, x)| w * x).sum::<f32>();
        }
        Ok(output)
    }
}

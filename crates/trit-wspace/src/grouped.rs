//! 分组三值层：**跨行**按幅度分层后分组共享 scale。
//!
//! 背景（实测，见 `docs/WSP-DIMSWEEP.md`）：
//! - 单 scale 三值化（per-tensor）在 STS 上掉 **24.91 点**——不是"三值精度不够"，而是
//!   同一组里混着幅度差上百倍的权重，统一的 `E|w|` 两头不讨好；
//! - **按幅度排序后每 G 个共享一个 scale**，代价降到 **0.0004 点**（全量 1379 对）；
//! - 但这一步需要记录"每个元素属于哪一组"（排序划分本身是额外信息）：
//!   每组 256、约 59 万权重 ⇒ 2304 组 ⇒ 约 11.2 bit/元素。
//!
//! 因此本结构的真实定位是：**在"权重必须是三值"的硬约束下**（可穷举验证、可逐位 diff），
//! 用 `2 + 32/G + log2(组数)` bit/权重 换取几乎无损——**不是**通用压缩方案
//! （int8 是 8 bit 且无需额外信息，压缩率更高）。
//!
//! 与 [`crate::net::Layer`] 的区别：`Layer` 的 `scales` 是**每行一个**；本结构的 `scales`
//! 是**每 `group_size` 个元素一个**，且分组来自跨行幅度排序，故额外需要 `groups`。

use crate::error::{Error, Result};
use crate::trit::Trit;

/// 分组三值层。
#[derive(Clone, Debug, PartialEq)]
pub struct GroupedLayer {
    rows: usize,
    cols: usize,
    group_size: usize,
    trits: Vec<Trit>,
    /// 每个元素所属的组号（解码必需）。
    groups: Vec<u32>,
    /// 每组的 scale。
    scales: Vec<f32>,
    bias: Vec<Trit>,
    bias_f32: Vec<f32>,
}

impl GroupedLayer {
    /// 由 f32 权重构造：按幅度排序后每 `group_size` 个共享一个 `E|w|`。
    ///
    /// 三值取 `sign(w)`，阈值 `0.7·E|w|`（组内均值绝对值），组内不超阈值的置 0。
    pub fn quantize(
        rows: usize,
        cols: usize,
        weights: &[f32],
        bias: &[f32],
        group_size: usize,
    ) -> Result<Self> {
        if rows == 0 || cols == 0 {
            return Err(Error::EmptyLayer { rows, cols });
        }
        if weights.len() != rows * cols {
            return Err(Error::Shape {
                what: "grouped weights",
                expected: rows * cols,
                got: weights.len(),
            });
        }
        if bias.len() != rows {
            return Err(Error::Shape {
                what: "grouped bias",
                expected: rows,
                got: bias.len(),
            });
        }
        let group_size = group_size.max(1);
        let count = weights.len();

        // 1) 按幅度升序得到划分顺序（这就是"排序"）
        let mut order: Vec<usize> = (0..count).collect();
        order.sort_by(|left, right| {
            weights[*left]
                .abs()
                .partial_cmp(&weights[*right].abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // 2) 每组一个 E|w| 作为 scale，并按阈值决定 trit
        let mut trits = vec![Trit::Zero; count];
        let mut groups = vec![0u32; count];
        let mut scales = Vec::with_capacity(count.div_ceil(group_size));
        for (group, chunk) in order.chunks(group_size).enumerate() {
            let mean_abs =
                chunk.iter().map(|index| weights[*index].abs()).sum::<f32>() / chunk.len() as f32;
            scales.push(mean_abs);
            let threshold = 0.7 * mean_abs;
            for index in chunk {
                groups[*index] = u32::try_from(group).unwrap_or(u32::MAX);
                let value = weights[*index];
                trits[*index] = if value > threshold {
                    Trit::Pos
                } else if value < -threshold {
                    Trit::Neg
                } else {
                    Trit::Zero
                };
            }
        }

        let bias_trits = bias
            .iter()
            .map(|value| {
                if *value > 0.0 {
                    Trit::Pos
                } else if *value < 0.0 {
                    Trit::Neg
                } else {
                    Trit::Zero
                }
            })
            .collect();

        Ok(Self {
            rows,
            cols,
            group_size,
            trits,
            groups,
            scales,
            bias: bias_trits,
            bias_f32: bias.to_vec(),
        })
    }

    /// 行数。
    #[must_use]
    pub fn rows(&self) -> usize {
        self.rows
    }

    /// 列数。
    #[must_use]
    pub fn cols(&self) -> usize {
        self.cols
    }

    /// 每组大小。
    #[must_use]
    pub fn group_size(&self) -> usize {
        self.group_size
    }

    /// 组数。
    #[must_use]
    pub fn group_count(&self) -> usize {
        self.scales.len()
    }

    /// `(row, col)` 处的 trit。
    #[must_use]
    pub fn trit(&self, row: usize, col: usize) -> Option<Trit> {
        if row >= self.rows || col >= self.cols {
            return None;
        }
        Some(self.trits[row * self.cols + col])
    }

    /// `(row, col)` 处所属的组号。
    #[must_use]
    pub fn group_of(&self, row: usize, col: usize) -> Option<u32> {
        if row >= self.rows || col >= self.cols {
            return None;
        }
        Some(self.groups[row * self.cols + col])
    }

    /// 第 `group` 组的 scale。
    #[must_use]
    pub fn scale(&self, group: usize) -> Option<f32> {
        self.scales.get(group).copied()
    }

    /// `(row, col)` 处的等效 f32 权重：`trit × scale[group]`。
    #[must_use]
    pub fn effective(&self, row: usize, col: usize) -> Option<f32> {
        let trit = self.trit(row, col)?;
        let group = *self.groups.get(row * self.cols + col)? as usize;
        let scale = self.scale(group)?;
        Some(trit.to_i8() as f32 * scale)
    }

    /// 每权重的实际位宽：`2`（trit）+ `32/G`（scale）+ `log2(组数)`（组号）。
    #[must_use]
    pub fn bits_per_weight(&self) -> f32 {
        let group_bits = if self.scales.len() <= 1 {
            0.0
        } else {
            (self.scales.len() as f32).log2()
        };
        2.0 + 32.0 / self.group_size as f32 + group_bits
    }

    /// 等效 f32 权重矩阵（行主序）——用于与 [`crate::net::Layer`] 对照。
    #[must_use]
    pub fn to_f32(&self) -> Vec<f32> {
        (0..self.rows * self.cols)
            .map(|index| {
                let row = index / self.cols;
                let col = index % self.cols;
                self.effective(row, col).unwrap_or(0.0)
            })
            .collect()
    }

    /// 一位前向：`y[r] = Σ_c trit×scale · x[c] + bias_f32[r]`。
    pub fn forward(&self, input: &[f32]) -> Result<Vec<f32>> {
        if input.len() != self.cols {
            return Err(Error::Shape {
                what: "grouped input",
                expected: self.cols,
                got: input.len(),
            });
        }
        let mut output = self.bias_f32.clone();
        for (row, slot) in output.iter_mut().enumerate() {
            let mut sum = 0.0f32;
            for (col, value) in input.iter().enumerate() {
                let index = row * self.cols + col;
                let scale = self.scales[self.groups[index] as usize];
                sum += self.trits[index].to_i8() as f32 * scale * value;
            }
            *slot += sum;
        }
        Ok(output)
    }
}

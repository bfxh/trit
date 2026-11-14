//! 分组三值层：**跨行**按幅度分层后分组共享 scale。
//!
//! 实测（见 `docs/WSP-DIMSWEEP.md`）：单 scale 三值化在 STS 上掉 24.91 点——不是"三值精度
//! 不够"，而是同组里混着幅度差上百倍的权重；**按幅度排序后每 G 个共享一个 scale**，
//! 代价降到 0.0004 点（全量 1379 对）。
//!
//! 但这需要记录"每个元素属于哪一组"（排序划分是额外信息）：每组 256、59 万权重 ⇒ 2304 组
//! ⇒ 约 11.2 bit/元素。定位：**在"权重必须是三值"的硬约束下**（可穷举验证、可逐位 diff），
//! 用 `2 + 32/G + log2(组数)` bit/权重 换取几乎无损——**不是**通用压缩方案。
//!
//! 与 [`crate::net::Layer`] 的关键差别：`Layer.scales` 是**每行一个**，故有"正 scale 不改
//! 行为"的不变性；本结构按**跨行**分组，同一行内不同列可落到不同 scale ⇒ 该不变性**失效**
//! （见 `tests/grouped_oracle.rs`），判等必须连 scale 一起固定。
//!
//! 运算（`effective` / `bits_per_weight` / `to_f32` / `forward`）在 [`crate::grouped_ops`]。

use crate::error::{Error, Result};
use crate::trit::Trit;

/// 分组三值层。
#[derive(Clone, Debug, PartialEq)]
pub struct GroupedLayer {
    pub(crate) rows: usize,
    pub(crate) cols: usize,
    pub(crate) group_size: usize,
    pub(crate) trits: Vec<Trit>,
    /// 每个元素所属的组号（解码必需）。
    pub(crate) groups: Vec<u32>,
    /// 每组的 scale。
    pub(crate) scales: Vec<f32>,
    pub(crate) bias: Vec<Trit>,
    pub(crate) bias_f32: Vec<f32>,
}

pub(crate) fn trit_of(value: f32) -> Trit {
    if value > 0.0 {
        Trit::Pos
    } else if value < 0.0 {
        Trit::Neg
    } else {
        Trit::Zero
    }
}

impl GroupedLayer {
    /// 由 f32 权重构造：按幅度排序后每 `group_size` 个共享一个 `E|w|`。
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
        let count = rows * cols;
        if weights.len() != count {
            return Err(Error::Shape {
                what: "grouped weights",
                expected: count,
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
        // 按幅度升序划分（"排序"就是这一步）
        let mut order: Vec<usize> = (0..count).collect();
        order.sort_unstable_by(|a, b| weights[*a].abs().total_cmp(&weights[*b].abs()));
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
        Ok(Self {
            rows,
            cols,
            group_size,
            trits,
            groups,
            scales,
            bias: bias.iter().map(|value| trit_of(*value)).collect(),
            bias_f32: bias.to_vec(),
        })
    }

    /// 直接由 trits / 组号 / scales 构造（解码入口，也用于精确构造测试实例）。
    pub fn from_trits(
        rows: usize,
        cols: usize,
        trits: &[i8],
        groups: &[u32],
        scales: &[f32],
        bias: &[f32],
    ) -> Result<Self> {
        if rows == 0 || cols == 0 {
            return Err(Error::EmptyLayer { rows, cols });
        }
        let count = rows * cols;
        if trits.len() != count || groups.len() != count {
            return Err(Error::Shape {
                what: "grouped trits/groups",
                expected: count,
                got: trits.len().min(groups.len()),
            });
        }
        if bias.len() != rows {
            return Err(Error::Shape {
                what: "grouped bias",
                expected: rows,
                got: bias.len(),
            });
        }
        let mut parsed = Vec::with_capacity(count);
        for value in trits {
            parsed.push(match value {
                -1 => Trit::Neg,
                0 => Trit::Zero,
                1 => Trit::Pos,
                other => return Err(Error::InvalidTrit(*other)),
            });
        }
        if let Some(group) = groups.iter().find(|g| **g as usize >= scales.len()) {
            return Err(Error::OutOfBounds {
                layer: 0,
                row: *group as usize,
                col: scales.len(),
            });
        }
        Ok(Self {
            rows,
            cols,
            group_size: scales.len().max(1),
            trits: parsed,
            groups: groups.to_vec(),
            scales: scales.to_vec(),
            bias: bias.iter().map(|value| trit_of(*value)).collect(),
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

    /// 全部组的 scale。
    #[must_use]
    pub fn scales(&self) -> &[f32] {
        &self.scales
    }

    /// 可写 scale（规范化用）。
    pub(crate) fn scales_mut(&mut self) -> &mut [f32] {
        &mut self.scales
    }
}

//! 分组结构的**更粗粒度判等不变量**：逐行正比例。
//!
//! ## 为什么是这个
//!
//! `Layer` 之所以能在判等时忽略 scale，是因为它每行一个正 scale：把某行整体乘一个正数，
//! 该行各项同比放大，**符号输出不变**。这个性质可以被推广成一条与结构无关的判据：
//!
//! ```text
//! 若存在 k_r > 0 使 W_B[r, :] = k_r · W_A[r, :]（对每一行），则 sign(W_B·x) = sign(W_A·x)。
//! ```
//!
//! 它把两种结构统一起来了：
//!
//! | 结构 | 改动 | 是否逐行比例 | 结论 |
//! |---|---|---|---|
//! | `Layer`（每行一个 scale） | 改某行 scale | **是** | scale 可忽略 |
//! | `GroupedLayer`（跨行分组） | 改某组 scale | **一般不是**（一组跨多行） | scale 必须固定 |
//!
//! 所以"分组破坏判等不变性"不是分组本身的怪癖，而是：**分组让可用的 scale 自由度不再对齐到行**。
//!
//! ## 这是一个充分条件，不是充要条件
//!
//! 逐行正比例 ⇒ 行为等价；反过来不成立（两层行为相同，也可能既不成比例、又不是同一个
//! `trits + groups + scales`）。要**严格**判等仍然得比 [`GroupedLayer::behavior`]；
//! 逐行比例的价值在于**便宜**（`O(rows×cols)`）且**可解释**（直接指出哪几行差了几倍）。
//!
//! [`GroupedLayer::behavior`]: crate::grouped_equal::GroupedLayer::behavior

use crate::grouped::GroupedLayer;

impl GroupedLayer {
    /// 若 `other` 的每一行都与 `self` 的对应行**成正比例**（比例 > 0），返回各行的比例；
    /// 否则 `None`。
    ///
    /// 比例存在 ⇒ 两层符号行为完全相同（这是本模块提供的**充分条件**）。
    #[must_use]
    pub fn row_proportional(&self, other: &Self) -> Option<Vec<f32>> {
        if self.rows() != other.rows() || self.cols() != other.cols() {
            return None;
        }
        let mut ratios = Vec::with_capacity(self.rows());
        for row in 0..self.rows() {
            let mut ratio: Option<f32> = None;
            for col in 0..self.cols() {
                let left = self.effective(row, col)?;
                let right = other.effective(row, col)?;
                if left == 0.0 {
                    // 零无法按比例变成非零，反之亦然
                    if right != 0.0 {
                        return None;
                    }
                    continue;
                }
                let candidate = right / left;
                if candidate <= 0.0 || !candidate.is_finite() {
                    return None;
                }
                match ratio {
                    Some(existing) if (existing - candidate).abs() > 1e-6 => return None,
                    None => ratio = Some(candidate),
                    _ => {}
                }
            }
            ratios.push(ratio.unwrap_or(1.0));
        }
        Some(ratios)
    }

    /// 两层的**每一行**是否恰为同一比例（即整体缩放）——这是 [`row_proportional`] 的
    /// 最粗特例，也是 `canonical()` 能消掉的那种冗余。
    ///
    /// [`row_proportional`]: GroupedLayer::row_proportional
    #[must_use]
    pub fn globally_proportional(&self, other: &Self) -> bool {
        match self.row_proportional(other) {
            Some(ratios) => {
                let first = ratios.first().copied().unwrap_or(1.0);
                ratios.iter().all(|r| (r - first).abs() < 1e-6)
            }
            None => false,
        }
    }
}

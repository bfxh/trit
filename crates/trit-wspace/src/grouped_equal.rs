//! [`GroupedLayer`] 的"scale 无关"判等。
//!
//! ## 先说边界：完全 scale 无关是不可能的
//!
//! `Layer` 之所以能在判等时忽略 scale，是因为它**每行一个正 scale**：整行同乘一个正数，
//! 行内各项的相对大小不变，符号输出不变。
//!
//! `GroupedLayer` 按**跨行**分组，同一行内不同列可以落到不同 scale 上。此时单独缩放某一组
//! 会改变行内各项的相对权重，**符号输出随之改变**（`tests/grouped_oracle.rs` 给出了反例）。
//! 所以"忽略 scale"这条路在分组结构下不成立。
//!
//! ## 能做的三件事
//!
//! 1. [`GroupedLayer::behavior`] —— 穷举 `3^vars` 个 trit 输入，取每行输出符号作为**行为
//!    签名**。判等直接比签名，与 scale 的具体表达无关（只要行为一致，怎么写的都算等价）。
//! 2. [`GroupedLayer::canonical`] —— 把所有 scale 除以 `scales[0]`，消除**唯一**的冗余自由
//!    度（全体 scale 同乘一个正数不改变行为）。两个层的 canonical 形式由 `trits + groups +
//!    归一化 scale` 唯一确定。
//! 3. [`GroupedLayer::is_row_grouped`] —— 判断是否"每行自成一组"。只有这种情况下才退化成
//!    `Layer` 语义（正 scale 缩放不改变行为，scale 可以完全不参与比较）。
//!
//! 三者合起来的实用判据：
//! - 要**严格**判等 → 比 [`GroupedLayer::behavior`] 签名（规模受 `3^vars` 限制）；
//! - 要**快速**判等且已知结构相同 → 比 [`GroupedLayer::canonical`] 形式；
//! - 只在 [`GroupedLayer::is_row_grouped`] 为真时，才可以像 `Layer` 那样忽略 scale。

use crate::error::{Error, Result};
use crate::grouped::GroupedLayer;

/// 穷举行为签名时允许的最大输入维数（`3^n` 规模）。
pub const MAX_VARS: usize = 8;

fn sign_i8(value: f32) -> i8 {
    if value > 1e-9 {
        1
    } else if value < -1e-9 {
        -1
    } else {
        0
    }
}

impl GroupedLayer {
    /// 行为签名：穷举 `3^vars` 个 trit 输入，返回每个输入下每行输出的符号（`-1/0/1`）。
    ///
    /// 这是**真正的 scale 无关判等依据**——只描述行为，不涉及 scale 怎么表达。
    /// `vars` 不得超过 [`MAX_VARS`]（`3^8 = 6561`），否则返回 `DomainTooLarge`。
    pub fn behavior(&self, vars: usize) -> Result<Vec<Vec<i8>>> {
        if vars > MAX_VARS {
            return Err(Error::DomainTooLarge {
                vars,
                limit: MAX_VARS,
            });
        }
        if vars > self.cols() {
            return Err(Error::Shape {
                what: "behavior vars",
                expected: self.cols(),
                got: vars,
            });
        }
        let total = 3usize.pow(vars as u32);
        let mut signature = Vec::with_capacity(total);
        for code in 0..total {
            let mut input = vec![0.0f32; self.cols()];
            let mut rest = code;
            for slot in input.iter_mut().take(vars) {
                *slot = match rest % 3 {
                    0 => -1.0,
                    1 => 0.0,
                    _ => 1.0,
                };
                rest /= 3;
            }
            let output = self.forward(&input)?;
            signature.push(output.iter().map(|value| sign_i8(*value)).collect());
        }
        Ok(signature)
    }

    /// 在 `vars` 维输入上穷举比较两个层的行为是否完全相同。
    ///
    /// 要求两者形状一致（否则直接判不等）。
    pub fn equivalent(&self, other: &Self, vars: usize) -> Result<bool> {
        if self.rows() != other.rows() || self.cols() != other.cols() {
            return Ok(false);
        }
        Ok(self.behavior(vars)? == other.behavior(vars)?)
    }

    /// 规范化：所有 scale 除以 `scales[0]`。
    ///
    /// 全体 scale 同乘一个正数不改变符号行为，所以这个商是**唯一**的 scale 冗余自由度；
    /// 归一化之后，`trits + groups + scales` 就唯一确定一个等价类。
    #[must_use]
    pub fn canonical(&self) -> Self {
        let mut normalized = self.clone();
        let base = self.scales().first().copied().unwrap_or(1.0);
        if base > 0.0 && base.is_finite() {
            for scale in normalized.scales_mut() {
                *scale /= base;
            }
        }
        normalized
    }

    /// 是否"每行自成一组"——此时退化为 [`crate::net::Layer`] 的语义
    /// （正 scale 缩放整行不改变符号，scale 可以不参与判等）。
    #[must_use]
    pub fn is_row_grouped(&self) -> bool {
        let cols = self.cols();
        for row in 0..self.rows() {
            let first = match self.group_of(row, 0) {
                Some(group) => group,
                None => return false,
            };
            for col in 1..cols {
                if self.group_of(row, col) != Some(first) {
                    return false;
                }
            }
            // 同一组不能跨行：检查该组的其它成员都在本行
            for row_other in 0..self.rows() {
                if row_other == row {
                    continue;
                }
                for col_other in 0..cols {
                    if self.group_of(row_other, col_other) == Some(first) {
                        return false;
                    }
                }
            }
        }
        true
    }
}

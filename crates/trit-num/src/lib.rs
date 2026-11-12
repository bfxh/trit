//! 数值原语：补偿求和、Welford 流式统计、最小二乘/岭回归、中位数与缺失插补。
//!
//! 定位（数据清洗 F 的底层件）：插补与清洗统计需要**确定性、可复算**的数值件，
//! 不能依赖“换个机器/换个顺序结果就漂”的朴素累加。
//!
//! 口径：
//! - 所有求和走 Neumaier 补偿（同输入同顺序 ⇒ 同结果，且比朴素累加稳定）；
//! - 最小二乘用正规方程 + Cholesky，**截距不参与正则化**；奇异矩阵返回结构化错误；
//! - 提供一份 **f32 参考实现**（`ols_f32`），与 f64 路径做同题对拍——f32 是“同比特精度”的对照，
//!   不是替代品；
//! - 输入含 `NaN`/`inf` 一律结构化报错，不静默传播。

#![forbid(unsafe_code)]

pub mod impute;
pub mod ols;
pub mod stats;
pub mod sums;

pub use impute::{impute_linear, impute_mean, impute_median};
pub use ols::{Fit, ols_f32, ols_f64, predict};
pub use stats::{mad, median};
pub use sums::{Welford, sum_neumaier};

/// 数值错误。
#[derive(Debug, thiserror::Error)]
pub enum NumError {
    /// 样本数不足。
    #[error("样本数为 {samples}，至少需要 {needed}")]
    NotEnoughSamples {
        /// 实际样本数。
        samples: usize,
        /// 需要的最小样本数。
        needed: usize,
    },
    /// 输入为空。
    #[error("输入为空")]
    Empty,
    /// 维度不匹配。
    #[error("维度不匹配：期望 {expected} 列，实际 {actual} 列")]
    Dimension {
        /// 期望列数。
        expected: usize,
        /// 实际列数。
        actual: usize,
    },
    /// 标签长度与样本数不一致。
    #[error("x 有 {x} 个样本，y 有 {y} 个")]
    LabelLength {
        /// 样本数。
        x: usize,
        /// 标签数。
        y: usize,
    },
    /// 含非有限值（向量输入时 `col` 为 0）。
    #[error("输入含非有限值：第 {row} 行第 {col} 列（{value}）")]
    NonFinite {
        /// 行下标。
        row: usize,
        /// 列下标（向量输入为 0）。
        col: usize,
        /// 具体的非有限值。
        value: f64,
    },
    /// 矩阵奇异（Cholesky 对角元过小）。
    #[error("矩阵奇异：对角元 {pivot:e} 太小（换岭回归或改基）")]
    Singular {
        /// 触发判定的对角元。
        pivot: f64,
    },
    /// 正则化系数非法。
    #[error("正则化系数必须为有限非负")]
    BadLambda,
}

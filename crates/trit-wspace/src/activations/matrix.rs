//! `WSPACT1` 激活矩阵读取（拆出以守住 `activations.rs` 行数预算）。

use std::path::Path;

use super::MAX_ELEMENTS;
mod limit;
pub use limit::read as read_matrix_with_limit;

/// 读取一个激活矩阵；返回 `(rows, cols, values)`。元素数上限是 `MAX_ELEMENTS`。
pub fn read_matrix(path: impl AsRef<Path>) -> Result<(usize, usize, Vec<f32>), String> {
    read_matrix_with_limit(path, MAX_ELEMENTS)
}

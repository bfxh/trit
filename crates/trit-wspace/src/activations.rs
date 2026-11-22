//! 真实激活矩阵的读取（`WSPACT1` 格式）。
//!
//! 文件布局：8 字节魔数 `WSPACT1\0` + `u32 rows` + `u32 cols` + `rows*cols` 个
//! f32（小端）。导出脚本 `examples/export_bias_activations.py` 只作对齐基线，
//! 不参与交付；本模块只负责把导出的矩阵读回 Rust。

/// 魔数。
pub const MAGIC: [u8; 8] = *b"WSPACT1\0";

/// 单文件元素上限（挡住误读巨大文件）。
pub const MAX_ELEMENTS: usize = 1 << 24;

mod mask;
mod matrix;
pub use mask::read_mask;
pub use matrix::{read_matrix, read_matrix_with_limit};

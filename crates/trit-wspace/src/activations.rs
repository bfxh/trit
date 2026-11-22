//! 真实激活矩阵的读取（`WSPACT1` 格式）。
//!
//! 文件布局：8 字节魔数 `WSPACT1\0` + `u32 rows` + `u32 cols` + `rows*cols` 个
//! f32（小端）。导出脚本 `examples/export_bias_activations.py` 只作对齐基线，
//! 不参与交付；本模块只负责把导出的矩阵读回 Rust。

use std::fs;
use std::path::Path;

/// 魔数。
pub const MAGIC: [u8; 8] = *b"WSPACT1\0";

/// 单文件元素上限（挡住误读巨大文件）。
pub const MAX_ELEMENTS: usize = 1 << 24;

/// 读取一个激活矩阵；返回 `(rows, cols, values)`。
pub fn read_matrix(path: impl AsRef<Path>) -> Result<(usize, usize, Vec<f32>), String> {
    let path = path.as_ref();
    let bytes = fs::read(path).map_err(|error| format!("读 {} 失败：{error}", path.display()))?;
    if bytes.len() < 16 {
        return Err(format!(
            "{} 太短（{} 字节，至少 16）",
            path.display(),
            bytes.len()
        ));
    }
    if bytes[..8] != MAGIC {
        return Err(format!("{} 不是 WSPACT1 格式", path.display()));
    }
    let rows = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize;
    let cols = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]) as usize;
    let elements = rows
        .checked_mul(cols)
        .ok_or_else(|| format!("{rows}×{cols} 溢出 usize"))?;
    if elements > MAX_ELEMENTS {
        return Err(format!("矩阵过大：{rows}×{cols} 超过 {MAX_ELEMENTS} 元素"));
    }
    let expected = 16usize
        .checked_add(elements.checked_mul(4).ok_or("字节数溢出")?)
        .ok_or("字节数溢出")?;
    if bytes.len() != expected {
        return Err(format!(
            "{} 长度 {} != 期望 {expected}（{rows}×{cols} f32）",
            path.display(),
            bytes.len()
        ));
    }
    let values = bytes[16..]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|chunk| f32::from_le_bytes(*chunk))
        .collect();
    Ok((rows, cols, values))
}

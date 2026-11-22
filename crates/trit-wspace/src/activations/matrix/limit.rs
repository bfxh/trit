//! 带显式上限的矩阵读取（从 `matrix.rs` 拆出，守住行数预算）。
use std::fs;
use std::path::Path;

use super::super::MAGIC;

/// 带显式上限的 [`read_matrix`]：全量语料（如 STS test 2758 句）会超过
/// [`MAX_ELEMENTS`]，调用方必须自己声明合法上限，默认护栏不放松。
pub fn read(path: impl AsRef<Path>, limit: usize) -> Result<(usize, usize, Vec<f32>), String> {
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
    if elements > limit {
        return Err(format!("矩阵过大：{rows}×{cols} 超过 {limit} 元素"));
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

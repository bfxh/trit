//! 函数边界的 checked 求和（从 mod.rs 拆出，控制单文件行数预算）。

use crate::elf;

/// checked 求和；溢出返回 `None`（调用方按不可信降级，不回绕、不饱和）。
pub(crate) fn checked_sum_u64(mut values: impl Iterator<Item = u64>) -> Option<u64> {
    values.try_fold(0u64, u64::checked_add)
}

/// 可执行节字节总和；不可信时把原因写进 notes，按 0 降级。
pub(crate) fn exec_bytes<'a>(exec: impl Iterator<Item = &'a [u8]>, notes: &mut Vec<String>) -> u64 {
    checked_sum_u64(exec.map(|data| data.len() as u64)).unwrap_or_else(|| {
        notes.push("可执行字节求和溢出：边界结论保持未知".to_string());
        0
    })
}

/// 函数符号 size 总和；不可信时把原因写进 notes，按 0 降级。
pub(crate) fn symbol_bytes(symbols: &[elf::FunctionSymbol], notes: &mut Vec<String>) -> u64 {
    checked_sum_u64(symbols.iter().map(|symbol| symbol.size)).unwrap_or_else(|| {
        notes.push("函数符号 size 求和溢出：不采信符号表精确边界".to_string());
        0
    })
}

pub(crate) fn frame_bytes(symbols: &[elf::FunctionSymbol]) -> Option<u64> {
    checked_sum_u64(symbols.iter().map(|symbol| symbol.size))
}

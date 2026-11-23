//! 指令地址 checked 计算（AArch64 固定 4 字节步长）。

pub(super) fn pc_at(base: u64, index: usize) -> Option<u64> {
    (index as u64)
        .checked_mul(4)
        .and_then(|offset| base.checked_add(offset))
}

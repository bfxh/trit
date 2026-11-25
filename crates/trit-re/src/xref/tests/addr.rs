//! `xref` 地址 checked 求和的回归（从 tests.rs 拆出，控制单文件行数预算）。

use super::super::insn_addr;

#[test]
fn insn_addr_legal_offsets_unchanged() {
    // 正例：合法节地址 + 指令下标与原朴素加法一致。
    assert_eq!(insn_addr(0x1000, 0), Some(0x1000));
    assert_eq!(insn_addr(0x1000, 0x10), Some(0x1010));
    assert_eq!(insn_addr(0, 0), Some(0));
    // 边界相邻：u64::MAX - 5 + 5 = MAX，不溢出。
    assert_eq!(insn_addr(u64::MAX - 5, 5), Some(u64::MAX));
}

#[test]
fn insn_addr_rejects_wrapping_addresses() {
    // 极端：近上限节地址 + 下标回绕 → `None`（跳过该指令，不回绕假地址）。
    assert_eq!(insn_addr(u64::MAX, 1), None);
    assert_eq!(insn_addr(u64::MAX - 3, 5), None);
    assert_eq!(insn_addr(u64::MAX - 1, usize::MAX), None);
}

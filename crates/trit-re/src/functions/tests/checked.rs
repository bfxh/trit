//! `functions` checked 求和的回归（从 tests.rs 拆出，控制单文件行数预算）。

use super::super::sum::checked_sum_u64;

#[test]
fn checked_sum_legal_lengths_unchanged() {
    // 正例：合法长度/大小求和与原 `sum()` 逐值一致。
    assert_eq!(checked_sum_u64([0u64, 1, 2, 3].into_iter()), Some(6));
    assert_eq!(checked_sum_u64([].into_iter()), Some(0));
    // 边界相邻：MAX - 1 + 1 = MAX，不溢出。
    assert_eq!(
        checked_sum_u64([u64::MAX - 1, 1].into_iter()),
        Some(u64::MAX)
    );
}

#[test]
fn checked_sum_rejects_overflowing_sizes() {
    // 极端：单个超大 size 与任何正值相加即溢出 → `None`（不回绕、不 panic）。
    assert_eq!(checked_sum_u64([u64::MAX, 1].into_iter()), None);
    assert_eq!(
        checked_sum_u64([u64::MAX / 2 + 1, u64::MAX / 2 + 1].into_iter()),
        None
    );
    assert_eq!(checked_sum_u64([u64::MAX, 0].into_iter()), Some(u64::MAX));
}

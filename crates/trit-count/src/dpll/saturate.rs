//! u128 饱和乘加（#591）：溢出记一次 `overflows` 并返回 `u128::MAX`，
//! 不再 debug panic / release wrapping。拆出以守住 `dpll.rs` 的行数预算。

use super::Counter;

impl Counter {
    /// 饱和乘法：溢出时记一次 `overflows` 并返回 `u128::MAX`。
    ///
    /// 旧实现在这里用裸 `*`：debug 直接 panic（exit 101），release 按补码回绕后
    /// 静默返回被截断的错值（exit 0），同一条输入两个构建给出两种结论（#591）。
    pub(super) fn mul(&mut self, left: u128, right: u128) -> u128 {
        match left.checked_mul(right) {
            Some(value) => value,
            None => {
                self.stats.overflows += 1;
                u128::MAX
            }
        }
    }

    /// 饱和加法，口径同 [`Counter::mul`]。
    pub(super) fn add(&mut self, left: u128, right: u128) -> u128 {
        match left.checked_add(right) {
            Some(value) => value,
            None => {
                self.stats.overflows += 1;
                u128::MAX
            }
        }
    }

    /// `2^exponent`，超出 `u128` 时饱和到 `u128::MAX`。
    pub(super) fn exp2(&mut self, exponent: usize) -> u128 {
        match u32::try_from(exponent)
            .ok()
            .and_then(|shift| 1u128.checked_shl(shift))
        {
            Some(value) => value,
            None => {
                self.stats.overflows += 1;
                u128::MAX
            }
        }
    }
}

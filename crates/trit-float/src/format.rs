//! 带指数范围的格式：精度 + 指数位数（平衡三进制）+ 可选的锥形精度。
//!
//! 这不是 IEEE 的替代品，而是一个**可比较的口径**：同样存储比特下，
//! 三进制把比特花在"精度"还是"范围"上，换来什么、失去什么。
//!
//! 指数用平衡三进制 `E` 位表示，范围是 `[-(3^E-1)/2, (3^E-1)/2]`——
//! 注意这是**指数本身的取值范围**，随 `E` 指数增长（3^E），不是线性的 2^E。

use crate::{LOG2_3, MAX_PRECISION, MIN_PRECISION, RoundMode, TritFloat};

/// `log10(3)`。
const LOG10_3: f64 = 0.477_121_254_719_662_4;

/// 一个三进制浮点格式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Format {
    /// 最大有效三进制位数。
    pub precision: u32,
    /// 指数占多少平衡三进制位。
    pub exponent_digits: u32,
    /// 锥形精度：每 `taper` 个绝对值指数减 1 位精度；0 = 固定精度。
    pub taper: u32,
}

impl Format {
    /// 构造（精度夹到 `[1, 40]`，指数位数夹到 `[0, 40]`）。
    pub fn new(precision: u32, exponent_digits: u32, taper: u32) -> Self {
        Self {
            precision: precision.clamp(MIN_PRECISION, MAX_PRECISION),
            exponent_digits: exponent_digits.min(40),
            taper,
        }
    }

    /// 存储位数（符号 1 + 尾数 + 指数，按 `log2 3` 折算）。
    pub fn total_bits(self) -> f64 {
        (self.precision + self.exponent_digits) as f64 * LOG2_3 + 1.0
    }

    /// 指数绝对值上限 `(3^E - 1) / 2`。
    pub fn max_exponent(self) -> i32 {
        let power = 3i128.checked_pow(self.exponent_digits).unwrap_or(i128::MAX);
        ((power.saturating_sub(1)) / 2).min(i32::MAX as i128) as i32
    }

    /// 在指数 `e` 处的有效精度（锥形时随 `|e|` 递减，下限 1）。
    pub fn precision_at(self, exponent: i32) -> u32 {
        if self.taper == 0 {
            return self.precision;
        }
        let drop = exponent.unsigned_abs() / self.taper;
        self.precision.saturating_sub(drop).max(MIN_PRECISION)
    }

    /// 可表示的最大正值。
    pub fn max_value(self) -> f64 {
        let max_exp = self.max_exponent();
        let limit = mantissa_limit(self.precision_at(max_exp)) as f64;
        limit * 3f64.powi(max_exp)
    }

    /// 最小的正规格化值（尾数 ±1）。
    pub fn min_positive(self) -> f64 {
        3f64.powi(-self.max_exponent())
    }

    /// 动态范围 `max / min`（可能溢出，统计用对数版本）。
    pub fn dynamic_range(self) -> f64 {
        self.max_value() / self.min_positive()
    }

    /// `log10(动态范围)`，避免大指数溢出。
    pub fn log10_range(self) -> f64 {
        let max_exp = self.max_exponent();
        let limit = mantissa_limit(self.precision_at(max_exp)) as f64;
        (limit.log10() + max_exp as f64 * LOG10_3) - (-(max_exp as f64) * LOG10_3)
    }

    /// 相对误差上界 `3^{1-p}` 的 `-log10`（越大越好）。
    pub fn log10_error(self) -> f64 {
        (self.precision as f64 - 1.0) * LOG10_3
    }

    /// 把 `value` 量化到本格式；溢出返回 `None`，下溢为 `Some(0)`。
    pub fn quantize(self, value: f64) -> Option<TritFloat> {
        self.quantize_with(value, RoundMode::HalfAway)
    }

    /// 同 [`quantize`](Self::quantize)，显式指定平局处理方式。
    pub fn quantize_with(self, value: f64, mode: RoundMode) -> Option<TritFloat> {
        if !value.is_finite() {
            return None;
        }
        if value == 0.0 {
            return Some(TritFloat::zero(self.precision));
        }
        let max_exp = self.max_exponent();
        // 先用最大精度定位指数，再按锥形精度重新量化。
        let probe = TritFloat::from_f64_with(value, self.precision, mode)?;
        let exponent = probe.exponent();
        if exponent > max_exp {
            return None;
        }
        if exponent < -max_exp {
            return Some(TritFloat::zero(self.precision));
        }
        let precision = self.precision_at(exponent);
        let quantized = TritFloat::from_f64_with(value, precision, mode)?;
        if quantized.exponent() > max_exp {
            return None;
        }
        Some(quantized)
    }
}

/// `p` 位平衡尾数的上界 `(3^p - 1)/2`。
fn mantissa_limit(precision: u32) -> i128 {
    let power = 3i128.checked_pow(precision).unwrap_or(i128::MAX);
    (power - 1) / 2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bits_add_up() {
        let f = Format::new(7, 3, 0);
        assert!((f.total_bits() - (10.0 * LOG2_3 + 1.0)).abs() < 1e-9);
    }

    #[test]
    fn exponent_range_is_ternary_exponential() {
        // E=1 → ±1；E=2 → ±4；E=3 → ±13。
        assert_eq!(Format::new(4, 1, 0).max_exponent(), 1);
        assert_eq!(Format::new(4, 2, 0).max_exponent(), 4);
        assert_eq!(Format::new(4, 3, 0).max_exponent(), 13);
    }

    #[test]
    fn quantize_clamps_to_format() {
        let f = Format::new(6, 2, 0);
        // 最大指数 4：3^5 已溢出。
        assert!(f.quantize(3f64.powi(5)).is_none());
        // 3^-5 下溢为 0。
        let under = f.quantize(3f64.powi(-5)).expect("有限值");
        assert!(under.is_zero());
        // 正常范围可量化。
        let ok = f.quantize(2.0).expect("有限值");
        assert!((ok.to_f64() - 2.0).abs() < 1e-9);
    }

    #[test]
    fn taper_reduces_precision_away_from_one() {
        let f = Format::new(8, 4, 3);
        assert_eq!(f.precision_at(0), 8);
        assert_eq!(f.precision_at(3), 7);
        assert_eq!(f.precision_at(9), 5);
        assert_eq!(f.precision_at(10_000), 1);
    }

    #[test]
    fn tapered_max_is_smaller_than_fixed() {
        let fixed = Format::new(8, 3, 0);
        let tapered = Format::new(8, 3, 2);
        assert!(tapered.max_value() < fixed.max_value());
        // 最小正值只由指数决定（尾数 ±1 永远可表示）。
        assert!((tapered.min_positive() - fixed.min_positive()).abs() < 1e-30);
    }

    #[test]
    fn round_modes_differ_on_ties() {
        // p=2 的整数网格是 1/2/3/4：2.5 是 2 与 3 的平局。
        let away = TritFloat::from_f64_with(2.5, 2, RoundMode::HalfAway).expect("有限值");
        let even = TritFloat::from_f64_with(2.5, 2, RoundMode::HalfEven).expect("有限值");
        assert!((away.to_f64() - 3.0).abs() < 1e-9, "HalfAway 应远离零");
        assert!((even.to_f64() - 2.0).abs() < 1e-9, "HalfEven 取偶尾数 2");
        // p=1 的网格是 ±3^e（尾数恒为 ±1，奇偶相同）：平局退化为 away。
        let p1 = TritFloat::from_f64_with(1.0 / 3.0, 1, RoundMode::HalfEven).expect("有限值");
        assert!((p1.to_f64() - 1.0 / 3.0).abs() < 1e-9);
    }
}
